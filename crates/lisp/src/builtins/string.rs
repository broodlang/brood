//! The string primitives: the pieces of the string library that genuinely need Rust —
//! Unicode case folding, normalization, grapheme and codepoint access, UTF-8 encoding,
//! substring/index scans at native speed, and `str`/`pr-str`. Everything else
//! (split/join/replace/trim/pad/…) is Brood over these in `std/string.blsp`.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_int, expect_string};
use super::realize_seqview;
use crate::core::heap::SlabRef;
use crate::core::value::StrId;
use crate::syntax::printer;

use super::numeric::{expect_string_ref, num_to_f64};
use super::sequences::realize_seqviews;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    primitives.def(
        "string/substring",
        Arity::range(2, 3),
        Sig::with_rest(vec![string, int], int, string),
        &["s", "start", "end"],
        "The characters of s in the range [start, end), char-indexed. end is optional and defaults to (string/length s), so (string/substring s start) is \"from start to the end\".\n\n    (string/substring \"Hi there\" 0 2)   → \"Hi\"",
        substring);
    primitives.def(
        "string/span",
        Arity::exact(3),
        Sig::new(vec![string, int, string], int),
        &["s", "start", "chars"],
        "The char index just past the maximal run of chars (a set, given as a string) starting at char `start` in s — `start` itself if the char there isn't in the set. The forward char-class scan a tokenizer skips a whitespace/digit run with; O(run) native. See also string/span-until.\n\n    (string/span \"  abc\" 0 \" \")   → 2\n    (string/span \"abc\" 0 \" \")   → 0",
        string_span);
    primitives.def(
        "string/span-until",
        Arity::exact(3),
        Sig::new(vec![string, int, string], int),
        &["s", "start", "chars"],
        "The char index of the first char of s in the set `chars` (a string) at or after char `start`, or (string/length s) if none — the maximal run of chars NOT in the set. For scanning up to a delimiter (comment-to-newline, atom-to-delimiter). The complement of string/span.\n\n    (string/span-until \"abc def\" 0 \" \")   → 3\n    (string/span-until \"abc\" 0 \" \")   → 3",
        string_span_until);
    // Linear substring search — like `substring`/`lower`, it genuinely needs Rust:
    // Brood has no O(1) char access (char indexing into UTF-8 is O(index)), so a
    // pure-Brood scan re-skips and is unavoidably O(n²) — which made `doc-search`'s
    // whole-namespace scan tens of seconds. `index-of` / `includes?`
    // (std/prelude.blsp) ride on this; it's the search counterpart of
    // the `substring` slice primitive.
    primitives.def(
        "%str-index-of",
        // Optional 3rd arg: the char index to start at, so `index-of`'s `from` can search
        // a suffix WITHOUT allocating one (see the fn's comment).
        Arity::range(2, 3),
        Sig::new(vec![string, string, int], int),
        &[],
        "",
        str_index_of,
    );
    // Reverse search: one forward pass with an advancing cursor. The Brood version called
    // `index-of` per match, and each of those re-derives a char offset, making a reverse
    // search over one string quadratic (measured 16.5x/16.4x per 4x). Editor hot path.
    primitives.def(
        "%str-last-index-of",
        Arity::range(2, 3),
        Sig::new(vec![string, string, int], int),
        &[],
        "",
        str_last_index_of,
    );
    // Splitting genuinely needs Rust for the same reason as the search above: a
    // pure-Brood split re-`substring`s the tail each step, and char-indexed substring
    // is O(index), so the whole split is O(n²) — a 174 KB `git ls-files` output took
    // ~840 ms in the editor's project-file scan. Rust's `str::split` is one O(n) pass.
    primitives.def(
        "string/split",
        Arity::range(1, 2),
        // the pieces are strings, and there is always at least one (`""` splits to `("")`)
        Sig::with_optional(vec![string], vec![string], Ty::list_of(string)),
        &["s", "&optional", "sep"],
        "Split s into a list of substrings on each occurrence of sep, in one O(n) pass. sep defaults to a single space, so (string/split \"1 1 +\") is the word split. An empty separator splits s into its individual characters.\n\n    (string/split \"a,b\" \",\")   → (\"a\" \"b\")",
        string_split);
    // Codepoint access needs Rust for the same reason as split/search: char
    // indexing into UTF-8 is O(index), and the pure-Brood construction
    // (`map string/char->int` over `string->list`) pays a 1-char string + a closure
    // call per char. One O(n) pass to the vector the text parsers index.
    primitives.def(
        "string/->codepoints",
        Arity::exact(1),
        Sig::new(vec![string], Ty::vector_of(int)),
        &["s"],
        "The characters of s as a vector of integer Unicode codepoints, in one O(n) pass — the random-access form text parsers index with nth and compare as ints. The inverse is (%codepoints->string codes), i.e. string/codepoints->.\n\n    (string/->codepoints \"Hi there\")   → [72 105 32 116 104 101 114 101]",
        string_to_codepoints);
    // The inverse. It had none until 2026-08-26, so every text parser in std/ rebuilt its
    // result with `(apply str (map int->char cs))` — a seq view, a closure per code point
    // making a one-character string, then an N-way variadic concat. Mechanism, not policy:
    // encoding a code point sequence as UTF-8 is not a rule Brood can express cheaply.
    primitives.def(
        "%codepoints->string",
        Arity::exact(1),
        Sig::new(vec![seqable], string),
        &["codes"],
        "A string from a vector, list or bytes of integer Unicode codepoints — the inverse of string/->codepoints, in one O(n) pass. Errors on a non-int or a value that is not a Unicode scalar.",
        codepoints_to_string);
    // Grapheme clusters + normalisation: UAX #29 / UAX #15 table lookups, not rules
    // Brood can express. The cluster is the unit a human calls "a character", so it
    // is what editor cursor motion steps by; normalisation is what makes text that
    // reads the same compare the same under Brood's byte-structural `=`.
    primitives.def(
        "string/->graphemes",
        Arity::exact(1),
        Sig::new(vec![string], Ty::vector_of(string)),
        &["s"],
        "The extended grapheme clusters of s as a vector of strings — the unit a human means by \"character\". \"é\" spelled e + U+0301 is two codepoints but one grapheme; a flag emoji is four codepoints and one grapheme. Step a cursor by this, not by codepoint (which splits clusters and corrupts text). The sibling of string/->codepoints; (apply str (string/->graphemes s)) is s.\n\n    (string/->graphemes \"Hi there\")   → [\"H\" \"i\" \" \" \"t\" \"h\" \"e\" \"r\" \"e\"]",
        string_to_graphemes);
    // The indexed grapheme accessors (ADR-159). `string->graphemes` alone made the
    // *documented-correct* cursor step — read the cluster at an index — cost a vector
    // of every cluster in the string, per keystroke. These walk to the index instead.
    primitives.def(
        "string/grapheme-count",
        Arity::exact(1),
        Sig::new(vec![string], int),
        &["s"],
        "How many extended grapheme clusters s has — the length a human means, and the exclusive upper bound for grapheme-at. One O(n) pass, no allocation.\n\n    (string/grapheme-count \"Hi there\")   → 8",
        grapheme_count);
    primitives.def(
        "string/grapheme-at",
        Arity::range(2, 3),
        // `default` is the OPTIONAL third argument the arity allows — declared here so
        // the checker can type it. `Sig::new` left it undeclarable (audit/sig-arity).
        Sig::with_optional(vec![string, int], vec![any], any),
        &["s", "i", "default"],
        "The i-th grapheme cluster of s as a string, or default (else nil) when i is out of range. The grapheme-indexed char-at: walks to i instead of materialising every cluster, so a cursor step is not O(line length).\n\n    (string/grapheme-at \"héllo\" 1)   → \"é\"",
        grapheme_at);
    primitives.def(
        "string/substring-graphemes",
        Arity::range(2, 3),
        // `end` is the OPTIONAL third argument the arity allows (audit/sig-arity).
        Sig::with_optional(vec![string, int], vec![int], string),
        &["s", "start", "end"],
        "The half-open grapheme-cluster range [start, end) of s (end optional = to the end), clamped to the ends. The grapheme-indexed substring — plain substring is codepoint-indexed and can slice a cluster in half.\n\n    (string/substring-graphemes \"héllo\" 1 3)   → \"él\"\n    (string/substring-graphemes \"héllo\" 3)   → \"lo\"",
        substring_graphemes);
    primitives.def(
        "string/normalize",
        Arity::exact(2),
        Sig::new(vec![string, kw], string),
        &["s", "form"],
        "s in Unicode normalization form, one of :nfc :nfd :nfkc :nfkd. Brood's = is byte-structural, so text that reads identically ('é' as U+00E9 vs U+0065 U+0301) compares unequal until normalized. Canonical (:nfc/:nfd) preserves meaning; compatibility (:nfkc/:nfkd) also folds presentation ('ﬁ' -> 'fi', '²' -> '2') — right for search and identifier matching, wrong for round-tripping text.\n\n    (string/length (string/normalize \"é\" :nfd))   → 2\n    (string/normalize \"ﬁ\" :nfkc)   → \"fi\"",
        string_normalize);
    // The minimal-splice diff of two strings — one O(n) byte pass, char-indexed
    // result. Needs Rust like the search/split above (no O(1) char access), and it
    // is per-keystroke hot: every process-hosted editor buffer diffs old->new text
    // at the loop tail (std/editor/buffer-client `text-splice` rides on this).
    primitives.def(
        "%str-splice-diff",
        Arity::exact(2),
        Sig::new(vec![string, string], vec_ty),
        &[],
        "",
        str_splice_diff,
    );
    // Case folding (Unicode tables) and parse-or-nil genuinely need Rust; the rest
    // of the string library (split/join/replace/index-of/trim/…) is Brood over
    // these + `substring`/`%str-index-of`/`str` (std/prelude.blsp).
    primitives.def(
        "string/upper",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "s upper-cased (Unicode-aware).\n\n    (string/upper \"Hi there\")   → \"HI THERE\"",
        upper,
    );
    primitives.def(
        "string/lower",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "s lower-cased (Unicode-aware).\n\n    (string/lower \"Hi there\")   → \"hi there\"",
        lower,
    );
    // Codepoint ↔ char and byte-level UTF-8 access — the primitives encoding
    // modules need that can't be written in Brood over `substring` alone.
    primitives.def(
        "string/char->int",
        Arity::exact(1),
        Sig::new(vec![string], int),
        &["s"],
        "Unicode codepoint of the first character of string s (identical to the byte value for ASCII).\n\n    (string/char->int \"A\")   → 65",
        char_to_int);
    primitives.def(
        "string/int->char",
        Arity::exact(1),
        Sig::new(vec![int], string),
        &["n"],
        "A 1-char string for Unicode codepoint n. Errors on an invalid codepoint.\n\n    (string/int->char 955)   → \"λ\"",
        int_to_char,
    );
    primitives.def(
        "%string->utf8-bytes",
        Arity::exact(1),
        Sig::new(vec![string], bytes_ty),
        &["s"],
        "The UTF-8 encoding of s as a bytes value.",
        string_to_utf8_bytes,
    );
    primitives.def(
        "%utf8-bytes->string",
        Arity::exact(1),
        // What `collect_bytes` accepts: bytes, a vector or list of ints, or nil (empty).
        Sig::new(
            vec![bytes_ty.union(Ty::of(Tag::Vector)).union(Ty::LIST)],
            string,
        ),
        &["bytes"],
        "Decode UTF-8 bytes (a bytes value, vector, or list of ints 0–255) into a string. Errors on invalid UTF-8.",
        utf8_bytes_to_string);
    // `->fixed` renders a number with a fixed count of decimals — the one
    // float→text op `str`/`pr-str` can't express (they print shortest round-trip
    // form, i.e. full f64 precision). `round-to` (a *number*) is Brood over floor.
    primitives.def(
        "%->fixed",
        Arity::exact(2),
        Sig::new(vec![num, int], string),
        &["x", "n"],
        "Render number x as a string with exactly n digits after the decimal point (rounded). n must be >= 0.",
        to_fixed);
    // value <-> text and I/O
    primitives.def(
        "str",
        Arity::any(),
        Sig::variadic(any, string),
        &["&", "xs"],
        "Concatenate the display forms of the arguments into one string.\n\n    (str \"a\" 1 :b)   → \"a1:b\"",
        str_concat,
    );
    primitives.def(
        "%string-join",
        Arity::exact(2),
        Sig::new(vec![string, seq], string),
        &[],
        "",
        string_join,
    );
    primitives.def(
        "pr-str",
        Arity::exact(1),
        Sig::new(vec![any], string),
        &["x"],
        "The readable (re-readable) text form of x — quoted and escaped, so it reads back. Unlike str, which renders for display: (str \"hi\") is 2 chars, (pr-str \"hi\") is 4.\n\n    (string/length (pr-str \"hi\"))   → 4",
        pr_str,
    );
    // symbols
    //
    // There is deliberately no `name` primitive here. The symbol/keyword → spelling
    // operation is `->string` (std/prelude/core.blsp, taken over by `defability
    // Display`): one name in the language, and a Brood one, since the sigil rule is
    // policy rather than mechanism. `name` had been a bare Rust builtin holding an
    // ordinary English noun at root; it is now a word a user's program may take.
    primitives.def(
        "symbol",
        Arity::exact(1),
        Sig::new(vec![string.union(sym).union(kw)], sym),
        &["x"],
        "Coerce a string, symbol, or keyword to the matching symbol (interning if needed).\n\n    (symbol \"ab\")   → ab",
        to_symbol,
    );
    primitives.def(
        "keyword",
        Arity::exact(1),
        Sig::new(vec![string.union(sym).union(kw)], kw),
        &["x"],
        "Coerce a string, symbol, or keyword to the matching keyword (interning if needed).\n\n    (keyword \"ab\")   → :ab",
        to_keyword,
    );
}

/// A string argument to a **char-indexed** builtin: its bytes, its cached char count, and
/// whether a char index is also a byte offset. All three come from one slot resolution
/// because every one of these builtins needs all three, and the text is **borrowed** —
/// an owned copy of the haystack per call is what made incremental search quadratic once
/// already (`expect_string` still does that at ~113 other sites).
struct StrArg<'h> {
    id: StrId,
    s: SlabRef<'h, str>,
    chars: usize,
    ascii: bool,
}

impl StrArg<'_> {
    /// Byte offset of char `ci`, clamped to the end. Arithmetic when a char index *is* a
    /// byte offset; otherwise through the slot's sparse char→byte index (ADR-213), which
    /// is a lookup plus a bounded walk rather than a walk from the start.
    #[inline]
    fn char_to_byte(&self, heap: &Heap, ci: usize) -> usize {
        if self.ascii {
            ci.min(self.s.len())
        } else {
            heap.str_char_to_byte(self.id, ci)
        }
    }

    /// The return direction: a byte-level `find`/`match_indices` result as the char index
    /// the language speaks. `b` must be a char boundary.
    #[inline]
    fn byte_to_char(&self, heap: &Heap, b: usize) -> usize {
        if self.ascii {
            b
        } else {
            heap.str_byte_to_char(self.id, b)
        }
    }
}

/// Require a string, as the [`StrArg`] the char-indexed builtins work through.
#[inline]
fn expect_str_arg<'h>(heap: &'h Heap, who: &str, v: Value) -> Result<StrArg<'h>, LispError> {
    match v {
        Value::Str(id) => {
            let (chars, ascii) = heap.str_metrics(id);
            Ok(StrArg {
                id,
                s: heap.string(id),
                chars,
                ascii,
            })
        }
        other => Err(LispError::wrong_type(heap, who, "string", other)),
    }
}

// ---------- value <-> text and I/O ----------

pub(super) fn str_concat(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let args = realize_seqviews(heap, env, args)?;
    let mut s = String::new();
    for &a in &args {
        s.push_str(&printer::display(heap, a));
    }
    Ok(heap.alloc_string(&s))
}

/// `(%string-join sep coll)` — the native fast path behind `join` for a string
/// separator. Walks `coll` once, appending each element's display form (the same
/// `str`/`join` use) with `sep` between adjacent elements into one pre-sized
/// buffer — no intermediate cons list and no `reverse` pass, which is what the
/// all-Brood `join` paid (≈2N cons cells built then reversed). `coll` is realised
/// via `seq_items` (list / vector / range; empty → `""`). Semantics match the
/// prelude `join`: display form per element, separator only between adjacent
/// elements, so a single-element collection has no trailing separator.
pub(super) fn string_join(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sep = match arg(args, 0) {
        s @ Value::Str(_) => printer::display(heap, s),
        v => return Err(LispError::wrong_type(heap, "%string-join", "string", v)),
    };
    // Streaming fast path for a lazy int range (`(string/join (range n) ",")`): format
    // each integer straight into the buffer in one pass — no intermediate Vec of
    // `Value`s, no per-element string allocation. The range stays immutable; this
    // only changes how its joined string is *constructed*.
    if let Value::Range(id) = arg(args, 1) {
        use std::fmt::Write;
        let (lo, hi, step) = heap.range_parts(id);
        let mut s = String::new();
        let mut first = true;
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            if !first {
                s.push_str(&sep);
            }
            first = false;
            let _ = write!(s, "{i}");
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        return Ok(heap.alloc_string(&s));
    }
    let items = heap.seq_items(arg(args, 1))?;
    // Rough pre-size (separators + a small per-element allowance) to avoid most
    // re-grows without a second display pass just to compute the exact length.
    let mut s = String::with_capacity(sep.len() * items.len().saturating_sub(1) + items.len() * 8);
    for (i, &item) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(&sep);
        }
        s.push_str(&printer::display(heap, item));
    }
    Ok(heap.alloc_string(&s))
}

pub(super) fn pr_str(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v = match arg(args, 0) {
        sv @ Value::SeqView(_) => realize_seqview(heap, env, sv)?,
        other => other,
    };
    let s = printer::print(heap, v);
    Ok(heap.alloc_string(&s))
}

/// `(symbol x)` — the symbol whose spelling is `x`. Accepts a string (intern as
/// a fresh-or-existing symbol), a symbol (identity), or a keyword (same spelling,
/// retagged as a symbol). The lenient inverse of `->string`; pairs with `keyword`.
pub(super) fn to_symbol(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(_) => Ok(v),
        Value::Keyword(s) => Ok(Value::symbol(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::symbol(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "symbol",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(keyword x)` — the keyword whose spelling is `x`. Accepts a string (intern),
/// a keyword (identity), or a symbol (same spelling, retagged as a keyword).
/// Mirrors `symbol`; the two share an interner so a keyword and a symbol with the
/// same spelling carry equal `Symbol` ids (the tag is the only distinction).
pub(super) fn to_keyword(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Keyword(_) => Ok(v),
        Value::Sym(s) => Ok(Value::keyword(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::keyword(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "keyword",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(string/substring s start [end])` — the characters of `s` in `[start, end)`,
/// char-indexed (consistent with `string-length`). `end` defaults to the
/// string's length, so `(string/substring s start)` is "from `start` to the end".
/// Errors if out of range.

pub(super) fn substring(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // The hot one: `char-at`, `starts-with?` and `ends-with?` are all Brood over this
    // (`std/prelude.blsp`), so its per-call cost is the floor for most string code. It had
    // three separate O(whole string) steps for what is usually a tiny result — an owned
    // `expect_string` copy, a `chars().count()` length, and a `chars().skip()` walk. With
    // a 216 KB haystack, `(string/char-at s 3)` cost ~11.5 µs and did not care that it was reading
    // the 4th character: measured with the CALL COUNT FIXED, the cost tracked the string's
    // size (1/6/23 ms as it grew 13.5k → 54k → 216k chars).
    let v = arg(args, 0);
    let start = expect_int(heap, "string/substring", arg(args, 1))?;
    let sub: String = {
        let h: &Heap = heap;
        let a = expect_str_arg(h, "string/substring", v)?;
        // The cached char count, O(1) — it used to be a `chars().count()` per call.
        let len = a.chars as i64;
        let end = match args.get(2) {
            Some(_) => expect_int(h, "string/substring", arg(args, 2))?,
            None => len,
        };
        if start < 0 || end < start || end > len {
            return Err(LispError::runtime(format!(
                "string/substring: range [{}, {}) out of bounds for length {}",
                start, end, len
            ))
            .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
        }
        // Both ends converted, so this is a direct slice — O(result) rather than O(end),
        // on multi-byte text as well. `chars().skip(start)` used to walk from byte 0 on
        // every call, which is what made a per-character scan quadratic off the ASCII path.
        let lo = a.char_to_byte(h, start as usize);
        let hi = a.char_to_byte(h, end as usize);
        a.s[lo..hi].to_string()
    };
    Ok(heap.alloc_string(&sub))
}

/// Shared body of `string-span` / `string-span-until`: from char `start`, count the
/// maximal run of chars whose membership in the set `chars` equals `in_set`, and
/// return the char index just past it. Char-indexed, like `substring`/`char-at`. The
/// forward char-class scan a tokenizer runs its inner loops on (skip a whitespace /
/// digit / delimiter run) — O(run) native instead of O(run) interpreted recursion.
pub(super) fn string_span_impl(
    args: &[Value],
    heap: &mut Heap,
    who: &str,
    in_set: bool,
) -> LispResult {
    // A tokenizer calls this once per token over one document, so an O(whole document)
    // step here is O(tokens x document) overall. It had three: the owned `expect_string`
    // copy, `chars().count()` for the length, and `chars().skip(start)`. Borrow, read the
    // cached count, and start from a byte offset the slot converts in O(1) (ASCII) or a
    // one-stride walk (multi-byte).
    let v = arg(args, 0);
    let start = expect_int(heap, who, arg(args, 1))?;
    let h: &Heap = heap;
    let a = expect_str_arg(h, who, v)?;
    let set = expect_string_ref(h, who, arg(args, 2))?;
    let len = a.chars as i64;
    if start < 0 || start > len {
        return Err(LispError::runtime(format!(
            "{}: start {} out of bounds for length {}",
            who, start, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let byte_start = a.char_to_byte(h, start as usize);
    let mut idx = start as usize;
    for c in a.s[byte_start..].chars() {
        if set.contains(c) == in_set {
            idx += 1;
        } else {
            break;
        }
    }
    Ok(Value::int(idx as i64))
}

/// `(string/span s start chars)` — the char index just past the maximal run of chars
/// drawn from the set `chars`, beginning at `start` (so `start` itself when the char
/// there isn't in the set). For skipping a run *of* a class — whitespace, digits.
pub(super) fn string_span(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string/span", true)
}

/// `(string/span-until s start chars)` — the char index of the first char in the set
/// `chars` at or after `start` (or the length if none): the maximal run of chars
/// *not* in the set. For scanning up to a delimiter — comment-to-newline,
/// atom-to-delimiter, string-body-to-quote.
pub(super) fn string_span_until(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string/span-until", false)
}

/// Lexical category of an atom token (a maximal run of non-delimiter chars), matching
/// `std/editor/highlight`'s `hl--atom-face` shape: a `:`-prefixed or `nil`/`true`/`false`
/// constant is a `keyword`; one that parses as an int/float (like `string/->number`) is a
/// `number`; anything else is a plain `symbol`. The head-position special-form vs call
/// distinction is left to the consumer (it needs the surrounding `(`).

/// Scan a `|…|` bar body from `from` (just past the opening `|`) to just past the
/// closing `|` — honouring `\|`/`\\` escapes — or to `n` if unterminated. Shared by
/// the two `scan-tokens` bar arms (symbol and keyword).
pub(super) fn scan_bar(chars: &[char], n: usize, from: usize) -> usize {
    let mut j = from;
    while j < n {
        match chars[j] {
            '\\' => j += 2,
            '|' => {
                j += 1;
                break;
            }
            _ => j += 1,
        }
    }
    j.min(n)
}

/// `(%str-index-of s needle)` — the 0-based **char** index of the first
/// occurrence of `needle` in `s`, or -1 if absent. Linear: Rust's byte-level
/// `str::find`, then a one-pass byte→char-index conversion of the prefix. The
/// empty needle matches at 0 (matching `index-of`'s contract). The search
/// primitive the Brood `index-of`/`includes?` ride on; see the

pub(super) fn str_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed, not owned: `expect_string` would copy the whole haystack per call, which
    // is the difference between a linear incremental search and a quadratic one.
    let h: &Heap = heap;
    let a = expect_str_arg(h, "%str-index-of", arg(args, 0))?;
    let needle = expect_string_ref(h, "%str-index-of", arg(args, 1))?;
    // Optional 3rd arg: the CHAR index to start searching at. It exists so `index-of`'s
    // `from` does not have to build `(string/substring coll from n)` first — that copy is what
    // made "incremental search" over one string quadratic, the same trap the comment
    // above `string-split`'s registration describes for splitting. Searching a suffix
    // must not allocate one.
    let start = match args.get(2) {
        None | Some(Value::Nil) => 0usize,
        Some(&v) => match v {
            Value::Int(n) => n.max(0) as usize,
            other => return Err(LispError::wrong_type(heap, "%str-index-of", "int", other)),
        },
    };
    // Char index → byte offset and back, both through the slot: O(1) for a pure-ASCII
    // string (where the two numbers are equal) and an indexed lookup plus a bounded walk
    // otherwise. That is what makes an incremental search over one string linear rather
    // than O(position) per call **in both encoding regimes** — a char-count cache alone
    // could only do it for ASCII, because its mechanism is the ASCII test itself.
    //
    // A start past the end converts to the end, so an out-of-range start simply finds
    // nothing (matching the clamp the Brood side used to do).
    let byte_start = if start == 0 {
        0
    } else {
        a.char_to_byte(h, start)
    };
    let idx = match a.s[byte_start..].find(&*needle) {
        Some(rel) => a.byte_to_char(h, byte_start + rel) as i64,
        None => -1,
    };
    Ok(Value::int(idx))
}

/// `(%str-last-index-of s needle before)` — the char index of the **last** occurrence of
/// `needle` in `s` starting strictly before char index `before`, or -1.
///
/// Genuinely needs Rust, for the same reason as `string-split` and the `from` offset above:
/// the Brood version walked forward calling `(index-of s needle i)` per match, and every one
/// of those re-derives a char offset (and, before that offset existed, allocated a copy of
/// the suffix) — so a reverse search was O(matches x length). Measured 16.5x then 16.4x per
/// 4x of input, where linear is 4x. This is one forward pass with an advancing cursor.
///
/// It is on an editor hot path in both directions: `buffer.blsp`'s reverse search runs over
/// whole buffer text, and `lineedit.blsp` finds the current line's start (`last-index-of
/// text "\n" p`) on every keystroke.
pub(super) fn str_last_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed, not owned — see `%str-index-of`.
    let h: &Heap = heap;
    let a = expect_str_arg(h, "%str-last-index-of", arg(args, 0))?;
    let needle = expect_string_ref(h, "%str-last-index-of", arg(args, 1))?;
    // Cached char count, O(1); `char_len` used to be a full scan.
    let char_len = a.chars;
    let before = match args.get(2) {
        None | Some(Value::Nil) => char_len as i64,
        Some(&v) => match v {
            Value::Int(n) => n,
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "%str-last-index-of",
                    "int",
                    other,
                ))
            }
        },
    };
    // The empty needle matches at every position 0..=len, so the last start strictly before
    // `before` is `before - 1` (clamped). Kept as an explicit branch, exactly as the Brood
    // version had it: the general scan below would loop forever on a zero-width match.
    if needle.is_empty() {
        return Ok(Value::int(if before <= 0 {
            -1
        } else if before > char_len as i64 {
            char_len as i64
        } else {
            before - 1
        }));
    }
    if before <= 0 {
        return Ok(Value::int(-1));
    }
    // Byte limit for `before` (clamped past-the-end to the whole string). A match may START
    // before the limit and extend past it — that is still a match, so the bound is on the
    // match's start, not on the slice searched.
    let limit = if before as usize >= char_len {
        a.s.len()
    } else {
        a.char_to_byte(h, before as usize)
    };
    let mut best: Option<usize> = None;
    for (b, _) in a.s.match_indices(&*needle) {
        if b >= limit {
            break;
        }
        best = Some(b);
    }
    Ok(Value::int(match best {
        Some(b) => a.byte_to_char(h, b) as i64,
        None => -1,
    }))
}

/// `(%str-splice-diff old new)` — the minimal single splice `[lo hi repl]` that
/// turns `old` into `new`: replace `old[lo, hi)` (0-based CHAR indices) with the
/// string `repl`. The common prefix and suffix are trimmed off (the suffix never
/// overlaps the prefix), so the span is minimal; equal strings give `[n n ""]`.
/// One native byte-level pass snapped to char boundaries. Genuinely needs Rust:
/// this runs per keystroke on every process-hosted editor buffer (the myedit
/// flip), where the pure-Brood per-char scan (fn call + `char-at` per char) cost
/// ~40 ms/keystroke on a 300-line buffer — ~100× this pass.
pub(super) fn str_splice_diff(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed, not owned: this runs per keystroke over the WHOLE buffer text (twice),
    // and `expect_string` copied both. The result is three small values, so the borrows
    // end before the allocation below — the pattern every convertible `expect_string`
    // site takes (`seam`: the ones that allocate per piece *while* scanning, like
    // `string-split` and `scan-tokens`, cannot use it).
    let h: &Heap = heap;
    let old = expect_string_ref(h, "%str-splice-diff", arg(args, 0))?;
    let new = expect_string_ref(h, "%str-splice-diff", arg(args, 1))?;
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    // Common byte prefix, snapped BACK to a char boundary in both (a boundary in
    // one is a boundary in the other: the prefixes are byte-identical).
    let mut p = ob.iter().zip(nb.iter()).take_while(|(a, b)| a == b).count();
    while p > 0 && !old.is_char_boundary(p) {
        p -= 1;
    }
    // Common byte suffix over the remainders (capped so it can't overlap the
    // prefix), snapped FORWARD (shrunk) to a char boundary in both.
    let max_suf = (ob.len() - p).min(nb.len() - p);
    let mut s = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(max_suf)
        .take_while(|(a, b)| a == b)
        .count();
    while s > 0 && !(old.is_char_boundary(ob.len() - s) && new.is_char_boundary(nb.len() - s)) {
        s -= 1;
    }
    let lo = old[..p].chars().count() as i64;
    let hi = lo + old[p..ob.len() - s].chars().count() as i64;
    let repl_str = new[p..nb.len() - s].to_string();
    drop((old, new));
    let repl = heap.alloc_string(&repl_str);
    Ok(heap.alloc_vector(vec![Value::int(lo), Value::int(hi), repl]))
}

/// `(string/split s &optional sep)` — split `s` into a list of substrings on each
/// occurrence of `sep`, in one O(n) pass. `sep` defaults to a single space: splitting
/// on words is what a bare `split` is reached for, and `(string/split line " ")` was
/// the separator spelled out at nearly every call site. An empty separator splits `s`
/// into its individual characters (1-char strings). Mirrors the semantics of the former
/// pure-Brood `string-split`/`string->list`, but without the O(n²) tail-substring rebuild.
pub(super) fn string_split(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string/split", arg(args, 0))?;
    let sep = match args.get(1) {
        Some(v) => expect_string(heap, "string/split", *v)?,
        None => " ".to_string(),
    };
    let out: Vec<Value> = if sep.is_empty() {
        s.chars()
            .map(|c| heap.alloc_string(&c.to_string()))
            .collect()
    } else {
        s.split(sep.as_str())
            .map(|part| heap.alloc_string(part))
            .collect()
    };
    Ok(heap.list_from_slice(&out))
}

/// `(string/->codepoints s)` — the characters of `s` as a **vector of integer Unicode
/// codepoints**, one O(n) pass. The random-access text-scanning primitive:
/// parsers (std/regex, std/json, std/encoding) index code points with O(1)
/// `nth` and compare them as ints. Building the same vector in Brood —
/// `(apply vector (map string/char->int (string/->list s)))` — costs a 1-char string
/// allocation per char plus a closure call per char, and measured ~40 % of the
/// whole regex benchmark. Like `string-split`/`string-span`, this is text-access
/// *mechanism*; the parsers themselves stay in Brood.
pub(super) fn string_to_codepoints(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Borrowed: the codepoints are ints, so nothing is allocated while the borrow is
    // live — one copy of the string saved per call, on the parsers' hot path.
    let codes: Vec<Value> = {
        let s = expect_string_ref(heap, "string/->codepoints", arg(args, 0))?;
        s.chars().map(|c| Value::int(c as i64)).collect()
    };
    Ok(heap.alloc_vector(codes))
}

/// `(%codepoints->string codes)` — a string from a sequence of integer Unicode code
/// points: the **inverse of `string/->codepoints`**, which until now had none.
///
/// Its absence was a real gap, not a convenience. `string/->codepoints` is a native that
/// every text parser in `std/` uses to get an indexable code vector — and every one of them
/// then rebuilt its result with `(apply str (map int->char cs))`, which allocates a seq
/// view, calls a closure per code point to make a **one-character string**, and then
/// concatenates N of those variadically. That is what `std/string.blsp`'s
/// `codepoints->` was, so `json`'s per-string assembly, the regex matcher's and the hex
/// encoder's all paid it. One O(n) pass into a `String` replaces the whole shape.
///
/// Accepts a vector or list (and a `bytes` value, where a byte *is* its code point), so it
/// mirrors what the parsers actually hold. A value that is not an integer, or is not a
/// Unicode scalar (negative, above U+10FFFF, or a surrogate in D800–DFFF), is a clean
/// error naming the offender — a surrogate cannot be a `char`, and letting one through
/// would either panic or silently produce U+FFFD.
pub(super) fn codepoints_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let arg0 = arg(args, 0);
    // Collect the ints first, so the string is built without a live heap borrow.
    let codes: Vec<i64> = match arg0 {
        Value::Bytes(id) => heap
            .bytes(id)
            .as_bytes()
            .iter()
            .map(|b| *b as i64)
            .collect(),
        Value::Vector(id) => heap
            .vector(id)
            .to_vec()
            .iter()
            .map(int_code)
            .collect::<Result<_, _>>()
            .map_err(|v| bad_codepoint(heap, v))?,
        Value::Pair(_) | Value::Nil => {
            let mut out = Vec::new();
            let mut cur = arg0;
            while let Value::Pair(id) = cur {
                let (h, t) = heap.pair(id);
                out.push(int_code(&h).map_err(|v| bad_codepoint(heap, v))?);
                cur = t;
            }
            out
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%codepoints->string",
                "vector, list or bytes of codepoint ints",
                other,
            ))
        }
    };
    let mut out = String::with_capacity(codes.len());
    for c in codes {
        match u32::try_from(c).ok().and_then(char::from_u32) {
            Some(ch) => out.push(ch),
            None => {
                return Err(LispError::runtime(format!(
                    "%codepoints->string: {c} is not a Unicode scalar value (0..=0x10FFFF, \
                     excluding the surrogates 0xD800..=0xDFFF)"
                )))
            }
        }
    }
    Ok(heap.alloc_string(&out))
}

/// The int in `v`, or `v` itself when it is not one — the error carries the offender so
/// [`codepoints_to_string`] can name it.
fn int_code(v: &Value) -> Result<i64, Value> {
    match v {
        Value::Int(n) => Ok(*n),
        other => Err(*other),
    }
}

fn bad_codepoint(heap: &Heap, v: Value) -> LispError {
    LispError::wrong_type(heap, "%codepoints->string", "codepoint int", v)
}

/// `(string/->graphemes s)` — the **extended grapheme clusters** of `s` as a vector
/// of strings, one O(n) pass. The sibling of `string/->codepoints`, and the unit a
/// human means by "character": `"é"` written as `e` + U+0301 is two code points but
/// one grapheme, and a flag emoji is four code points and one grapheme. Cursor
/// motion, column arithmetic and truncation in `std/editor/*` all want this unit —
/// stepping a cursor by code point splits a cluster and corrupts the text. Not
/// bootstrappable: the boundary rules are UAX #29 tables, not a rule Brood can
/// express. `string/display-width` already segments the same way internally.
pub(super) fn string_to_graphemes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let s = expect_string(heap, "string/->graphemes", arg(args, 0))?;
    // `true` = *extended* grapheme clusters (UAX #29's recommended default, and
    // what the renderer and `string/display-width` use).
    let parts: Vec<String> = s.graphemes(true).map(|g| g.to_string()).collect();
    let vals: Vec<Value> = parts.iter().map(|g| heap.alloc_string(g)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(string/grapheme-count s)` — how many **extended grapheme clusters** `s` has: the
/// length a human means, and the exclusive upper bound for `grapheme-at`. One O(n)
/// segmentation pass that allocates nothing (`string->graphemes` had to build a
/// vector of n strings just to be counted).
pub(super) fn grapheme_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let s = expect_string_ref(heap, "string/grapheme-count", arg(args, 0))?;
    Ok(Value::int(s.graphemes(true).count() as i64))
}

/// `(string/grapheme-at s i)` / `(string/grapheme-at s i default)` — the `i`-th grapheme cluster
/// of `s` as a string, or `default`/`nil` when `i` is out of range (never an error,
/// matching `nth`/`get`).
///
/// Why this is a primitive and not `(nth (string/->graphemes s) i)`: the docs require
/// a cursor to step by *cluster*, so that spelling was the only correct way to read
/// one character — and it builds a vector of every cluster in the string on **every
/// keystroke**. This walks to `i` and stops, allocating one string. The editor's
/// hottest path stops being O(n) in the buffer line's length.
pub(super) fn grapheme_at(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let i = expect_int(heap, "string/grapheme-at", arg(args, 1))?;
    let default = args.get(2).copied().unwrap_or(Value::nil());
    if i < 0 {
        return Ok(default);
    }
    // Borrowed — the editor reads a cluster per keystroke, so a copy of the line (or the
    // buffer) per call is exactly what this path cannot afford.
    let found = {
        let s = expect_string_ref(heap, "string/grapheme-at", arg(args, 0))?;
        s.graphemes(true).nth(i as usize).map(|g| g.to_string())
    };
    match found {
        Some(g) => Ok(heap.alloc_string(&g)),
        None => Ok(default),
    }
}

/// `(string/substring-graphemes s start)` / `(… s start end)` — the half-open cluster range
/// `[start, end)` of `s` as a string, clamped to the ends (so it never errors, like
/// `take`/`drop`). The grapheme-indexed counterpart of `substring`, which is
/// codepoint-indexed and will happily slice a cluster in half — splitting `"é"`
/// (e + U+0301) into a bare `e` and an orphan combining mark.
pub(super) fn substring_graphemes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_segmentation::UnicodeSegmentation;
    let start = expect_int(heap, "string/substring-graphemes", arg(args, 1))?.max(0) as usize;
    let end = match args.get(2) {
        None | Some(Value::Nil) => None,
        Some(_) => {
            Some(expect_int(heap, "string/substring-graphemes", arg(args, 2))?.max(0) as usize)
        }
    };
    let out: String = {
        let s = expect_string_ref(heap, "string/substring-graphemes", arg(args, 0))?;
        match end {
            Some(e) if e <= start => String::new(),
            Some(e) => s.graphemes(true).skip(start).take(e - start).collect(),
            None => s.graphemes(true).skip(start).collect(),
        }
    };
    Ok(heap.alloc_string(&out))
}

/// `(string/normalize s form)` — `s` in Unicode normalisation `form`, one of the
/// keywords `:nfc` `:nfd` `:nfkc` `:nfkd`. Text that a human reads as identical can
/// be several different strings — "é" is U+00E9 *or* U+0065 U+0301 — and Brood's `=`
/// is byte-structural, so only normalisation makes those compare equal. Canonical
/// (`nfc`/`nfd`) preserves meaning; compatibility (`nfkc`/`nfkd`) also folds
/// presentation differences (the ligature "ﬁ" → "fi", superscript "²" → "2"), which
/// is right for search and identifier matching and wrong for round-tripping text.
/// One primitive with a form keyword rather than four functions (ADR-011).
pub(super) fn string_normalize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use unicode_normalization::UnicodeNormalization;
    let s = expect_string(heap, "string/normalize", arg(args, 0))?;
    let form = arg(args, 1);
    let name = match form {
        Value::Keyword(k) => crate::core::value::symbol_name(k),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "string/normalize",
                "keyword",
                form,
            ))
        }
    };
    let out: String = match name.as_str() {
        "nfc" => s.nfc().collect(),
        "nfd" => s.nfd().collect(),
        "nfkc" => s.nfkc().collect(),
        "nfkd" => s.nfkd().collect(),
        other => {
            return Err(LispError::runtime(format!(
                "string/normalize: unknown form :{other} (expected :nfc, :nfd, :nfkc or :nfkd)"
            )))
        }
    };
    Ok(heap.alloc_string(&out))
}

/// `(math/->fixed x n)` — x rendered with exactly `n` digits after the decimal point
/// (rounded). The one float→text op the language can't bootstrap: `str`/`pr-str`
/// print the shortest round-tripping form (full f64 precision, e.g.
/// `0.015873015873015872`), which is wrong for tabular/console output. An int `x`
/// is promoted, so `(math/->fixed 3 2)` is `"3.00"`. `n` must be non-negative.
pub(super) fn to_fixed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = num_to_f64(heap, "->fixed", arg(args, 0))?;
    let n = expect_int(heap, "->fixed", arg(args, 1))?;
    if n < 0 {
        return Err(LispError::runtime(format!(
            "->fixed: decimal places must be non-negative, got {}",
            n
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    // Bound the width: `format!("{:.*}", n, x)` materialises an `n`-digit string,
    // so an unbounded `n` (e.g. `(math/->fixed 1.0 1000000000)`) allocates ~1 GB on the
    // Rust side, bypassing the GC/soft-memory cap. An f64 carries ~17 significant
    // digits; past that the tail is just zeros, so 1000 is far beyond any real use
    // while keeping the worst-case alloc to ~1 KB.
    const MAX_DECIMALS: i64 = 1000;
    if n > MAX_DECIMALS {
        return Err(LispError::runtime(format!(
            "->fixed: decimal places {n} too large (math/max {MAX_DECIMALS}); an f64 has \
             ~17 significant digits, so a larger count only pads zeros"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let s = format!("{:.*}", n as usize, x);
    Ok(heap.alloc_string(&s))
}

/// `(string/upper s)` — `s` with every character upper-cased. Case folding is
/// Unicode-aware (e.g. `ß` → `SS`), so it leans on the standard library's tables
/// rather than being expressible in Brood.
pub(super) fn upper(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string_ref(heap, "string/upper", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_uppercase()))
}

/// `(string/lower s)` — `s` with every character lower-cased (Unicode-aware, like `upper`).
pub(super) fn lower(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string_ref(heap, "string/lower", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_lowercase()))
}

pub(super) fn char_to_int(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string/char->int", arg(args, 0))?;
    match s.chars().next() {
        Some(c) => Ok(Value::int(c as i64)),
        None => Err(LispError::runtime("string/char->int: empty string")),
    }
}

pub(super) fn int_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "string/int->char", arg(args, 0))?;
    // Guard the u32 range *before* the cast: `n as u32` would silently truncate a
    // value outside [0, u32::MAX] and could alias a valid codepoint (returning the
    // wrong char) instead of erroring.
    let c = u32::try_from(n)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| {
            LispError::runtime(format!(
                "string/int->char: {} is not a valid Unicode codepoint",
                n
            ))
        })?;
    let mut buf = [0u8; 4];
    Ok(heap.alloc_string(c.encode_utf8(&mut buf)))
}

pub(super) fn string_to_utf8_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->utf8-bytes", arg(args, 0))?;
    let bytes = s.as_bytes().to_vec();
    Ok(super::bytes::bytes_to_value(&bytes, heap))
}

pub(super) fn utf8_bytes_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Accepts a `bytes` value, or (leniently) a vector or list of byte ints.
    let bytes = super::bytes::collect_bytes("utf8-bytes->string", arg(args, 0), heap)?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(heap.alloc_string(&s)),
        Err(e) => Err(LispError::runtime(format!(
            "utf8-bytes->string: invalid UTF-8: {}",
            e
        ))),
    }
}
