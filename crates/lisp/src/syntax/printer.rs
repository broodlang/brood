//! Renders a [`Value`] to text. Needs the [`Heap`] to read heap objects.
//!
//! - [`print`] is *readable* (strings quoted/escaped) — what the REPL shows.
//! - [`display`] is *human* (strings raw) — what `print`/`str` use.

use crate::core::heap::Heap;
use crate::core::value::{Value, ValueRef, VecId, symbol_name_ref};

/// Maximum nesting the printer will descend into. Past this we stop
/// recursing — a printed REPL value should never be the thing that overflows
/// the native Rust stack. (A value built by `cons`-ing in a loop can be far
/// deeper than any reader could parse.)
///
/// It is **one more** than the reader's cap, and that `+ 1` is load-bearing:
/// the reader accepts `reader::MAX_DEPTH` nesting *levels*, and the innermost
/// element of such a form sits one printer level below the outermost list — so
/// with equal caps the printer truncated the deepest element of a form the
/// reader had just accepted (a 256-deep form printed to a *different* value,
/// with no error; 255 round-tripped). Anything the reader accepts must print
/// readably.
const MAX_DEPTH: u32 = crate::syntax::reader::MAX_DEPTH + 1;

/// Write `s` as a Brood string literal, quotes included, that re-reads to `s` — the one
/// escaping rule, shared by the printer and every Rust site that splices a string into
/// Brood source (`introspect::string_literal`).
pub fn write_string_literal(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // Match the reader's own spellings for ESC and NUL
            // (scanner::scan_string_body decodes `\e`/`\0`); any *other* C0 control
            // char (and DEL) has no named escape, so emit the reader's `\u{H..H}` form
            // rather than a raw byte. Readable output must round-trip.
            '\u{1b}' => out.push_str("\\e"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => {
                out.push_str("\\u{");
                out.push_str(&format!("{:x}", c as u32));
                out.push('}');
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

pub fn print(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    write_value(&mut out, heap, v, true, 0);
    out
}

pub fn display(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    display_into(&mut out, heap, v);
    out
}

/// [`display`] appended to `out` — for a caller assembling one string from several
/// values (`str`, `string/join`), which would otherwise build, copy and drop a `String`
/// per value (a full copy of every string argument).
pub fn display_into(out: &mut String, heap: &Heap, v: Value) {
    write_value(out, heap, v, false, 0);
}

fn write_value(out: &mut String, heap: &Heap, v: Value, readable: bool, depth: u32) {
    if depth >= MAX_DEPTH {
        // Past the cap, stop descending. In *readable* output a bare `…` would
        // re-read as the symbol `…` — a truncation indistinguishable from real
        // data — so emit an unreadable marker instead: `#` is a reader dispatch
        // character, so `#<deep>` is a hard parse error rather than a silently
        // wrong value. Human output keeps the compact ellipsis.
        out.push_str(if readable { "#<deep>" } else { "…" });
        return;
    }
    match v.unpack() {
        ValueRef::Nil => out.push_str("nil"),
        ValueRef::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        // Formatted straight into `out` — no temporary `String` per integer.
        ValueRef::Int(n) => {
            let _ = std::fmt::Write::write_fmt(out, format_args!("{n}"));
        }
        // A bignum prints as its decimal string, exactly like an `Int`.
        ValueRef::BigInt(id) => out.push_str(&heap.bigint(id).to_string()),
        // A decimal prints as its canonical decimal string with the `M` suffix so
        // it round-trips through the reader (e.g. `1.50M`). Both readable and
        // display use this form.
        ValueRef::Decimal(id) => {
            out.push_str(&heap.decimal(id).to_string());
            out.push('M');
        }
        // A ratio prints as `num/den` (e.g. `1/2`), which re-reads to the same
        // value — `BigRational`'s `Display` is exactly that form, and it is always
        // reduced with a positive denominator. Both readable and display use it.
        ValueRef::Ratio(id) => out.push_str(&heap.ratio(id).to_string()),
        // Raw bytes print as a `#b"…"` literal that re-reads to the same value:
        // printable ASCII verbatim, the named escapes by name, every other byte as
        // `\xHH`. Both readable and display use this form — bytes have no raw text.
        ValueRef::Bytes(id) => {
            out.push_str("#b\"");
            for &byte in heap.bytes(id).as_bytes() {
                match byte {
                    b'"' => out.push_str("\\\""),
                    b'\\' => out.push_str("\\\\"),
                    b'\n' => out.push_str("\\n"),
                    b'\t' => out.push_str("\\t"),
                    b'\r' => out.push_str("\\r"),
                    0x1b => out.push_str("\\e"),
                    0x00 => out.push_str("\\0"),
                    0x20..=0x7e => out.push(byte as char),
                    _ => out.push_str(&format!("\\x{:02x}", byte)),
                }
            }
            out.push('"');
        }
        ValueRef::Float(f) => out.push_str(&format_float(f)),
        ValueRef::Sym(s) => write_symbol(out, symbol_name_ref(s), readable, false),
        ValueRef::Keyword(s) => {
            out.push(':');
            write_symbol(out, symbol_name_ref(s), readable, true);
        }
        ValueRef::Str(id) => {
            let s = heap.string(id);
            if readable {
                write_string_literal(out, s);
            } else {
                out.push_str(s);
            }
        }
        ValueRef::Pair(_) => write_list(out, heap, v, readable, depth),
        // A range prints as the list it stands in for: `(0 1 2 3 4)`.
        ValueRef::Range(id) => {
            out.push('(');
            write_range_items(out, heap, id, readable, depth, &mut true);
            out.push(')');
        }
        // A lazy seq-view can't be realised here (the printer has no evaluator to
        // run its transducer). The prelude print path realises a view first, so
        // in normal use this is unreachable; print an opaque marker as the
        // never-panic fallback for an escaped raw view (e.g. inside a kernel
        // error message).
        ValueRef::SeqView(_) => out.push_str("#<seq-view>"),
        ValueRef::Vector(id) => {
            out.push('[');
            for (i, &item) in heap.vector(id).iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_value(out, heap, item, readable, depth + 1);
            }
            out.push(']');
        }
        ValueRef::Map(id) => {
            out.push('{');
            for (i, (k, v)) in heap.map_entries(id).iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, heap, *k, readable, depth + 1);
                out.push(' ');
                write_value(out, heap, *v, readable, depth + 1);
            }
            out.push('}');
        }
        // A failure prints as `#failure{…}` — self-identifying, and NOT re-readable,
        // which is deliberate: a failure records that something could not be produced,
        // so reconstructing one from text would manufacture a fake provenance.
        ValueRef::Failure(id) => {
            out.push_str("#failure{");
            let mut first = true;
            for (k, val) in heap.map_entries(id) {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                write_value(out, heap, k, readable, depth + 1);
                out.push(' ');
                write_value(out, heap, val, readable, depth + 1);
            }
            out.push('}');
        }
        // `#point{:x 1, :y 2}` (`#geo/point{…}` for a module's record): the id, then the
        // fields as a map prints them. Never a map literal, so printing a record and reading
        // the text back cannot make a map pose as one (ADR-542).
        ValueRef::Record { id: rid, fields } => {
            out.push('#');
            out.push_str(&crate::core::value::symbol_name(rid));
            out.push('{');
            let mut first = true;
            for (k, val) in heap.map_entries(fields) {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                write_value(out, heap, k, readable, depth + 1);
                out.push(' ');
                write_value(out, heap, val, readable, depth + 1);
            }
            out.push('}');
        }
        ValueRef::Set(id) => {
            // `#{a b c}` — elements only (the backing values are all `true`), in the
            // CHAMP's insertion order. Reads back as a set literal.
            out.push_str("#{");
            for (i, (k, _v)) in heap.map_entries(id).iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_value(out, heap, *k, readable, depth + 1);
            }
            out.push('}');
        }
        ValueRef::Fn(id) => {
            out.push_str("#<fn");
            if let Some(name) = heap.closure(id).name {
                out.push(' ');
                out.push_str(symbol_name_ref(name));
            }
            out.push('>');
        }
        ValueRef::Macro(id) => {
            out.push_str("#<macro");
            if let Some(name) = heap.closure(id).name {
                out.push(' ');
                out.push_str(symbol_name_ref(name));
            }
            out.push('>');
        }
        ValueRef::Native(id) => {
            out.push_str("#<native ");
            out.push_str(&heap.native(id).name);
            out.push('>');
        }
        ValueRef::Ref(n) => {
            out.push_str("#<ref ");
            out.push_str(&n.to_string());
            out.push('>');
        }
        ValueRef::Pid { node, id } => {
            out.push_str("#<pid ");
            out.push_str(symbol_name_ref(node));
            out.push('/');
            out.push_str(&id.to_string());
            out.push('>');
        }
        ValueRef::Rope(id) => {
            // A buffer rope can hold a whole file; print a summary, not the
            // text (use `rope->string` to get the content). Same for both
            // readable and display forms — a rope has no re-readable literal.
            let r = heap.rope(id);
            out.push_str("#<rope :chars ");
            out.push_str(&r.len_chars().to_string());
            out.push_str(" :lines ");
            out.push_str(&r.len_lines().to_string());
            out.push('>');
        }
        ValueRef::Socket(id) => {
            // A socket is a live OS resource with no readable literal.
            out.push_str("#<socket ");
            out.push_str(&id.to_string());
            out.push('>');
        }
        ValueRef::Subprocess(id) => {
            // A child process is a live OS resource with no readable literal.
            out.push_str("#<subprocess ");
            out.push_str(&id.to_string());
            out.push('>');
        }
        ValueRef::Table(id) => {
            // A shared in-memory table — no readable literal (identity handle).
            out.push_str("#<table ");
            out.push_str(&id.to_string());
            out.push('>');
        }
    }
}

fn write_list(out: &mut String, heap: &Heap, v: Value, readable: bool, depth: u32) {
    out.push('(');
    let mut cur = v;
    let mut first = true;
    // The list *spine* is iterated, not recursed (so a million-long proper
    // list doesn't overflow); only each `head` advances the depth counter.
    loop {
        match cur.unpack() {
            ValueRef::Pair(p) => {
                if !first {
                    out.push(' ');
                }
                first = false;
                let (head, tail) = heap.pair(p);
                write_value(out, heap, head, readable, depth + 1);
                cur = tail;
            }
            ValueRef::Nil => break,
            // A lazy range in the TAIL stands for the list it denotes, so it
            // splices into the spine — `(cons 9 (range 2))` is `(9 0 1)`, not the
            // dotted `(9 . (0 1))` — which would not read back as itself, and
            // `=`/`count` treat the spine as proper.
            ValueRef::Range(id) => {
                write_range_items(out, heap, id, readable, depth, &mut first);
                break;
            }
            other => {
                out.push_str(" . ");
                write_value(out, heap, other, readable, depth + 1);
                break;
            }
        }
    }
    out.push(')');
}

/// Write a lazy range's elements as list items, space-separated after the first
/// (`first` carries across, so a range in a list's tail splices into its spine).
fn write_range_items(
    out: &mut String,
    heap: &Heap,
    id: VecId,
    readable: bool,
    depth: u32,
    first: &mut bool,
) {
    let (lo, hi, step) = heap.range_parts(id);
    let mut i = lo;
    while if step > 0 { i < hi } else { i > hi } {
        if !*first {
            out.push(' ');
        }
        *first = false;
        write_value(out, heap, Value::int(i), readable, depth + 1);
        // A range ending near `i64::MAX`/`MIN` would wrap here (and loop forever in
        // release): the next element past the edge does not exist.
        i = match i.checked_add(step) {
            Some(next) => next,
            None => break,
        };
    }
}

pub(crate) fn format_float(f: f64) -> String {
    // Non-finite floats print as `inf`/`-inf`/`nan` — which the READER parses
    // back as float literals (see float_roundtrip_test.blsp), so the round-trip
    // holds. Any change here must be checked through the FULL round trip.
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    // `{:?}` is Rust's shortest ROUND-TRIP rendering and uses scientific notation
    // for extreme magnitudes — `{}` (Display) never does, so 1e300 printed as a
    // 300-digit expansion (same finding). Both keep a trailing `.0` on integral
    // floats; the suffix fix-up below is belt-and-braces.
    let s = format!("{:?}", f);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{}.0", s)
    }
}

/// Emit a symbol/keyword `name` (the keyword's `:` is written by the caller),
/// bar-quoting it as `|…|` when `readable` and the bare name wouldn't read back as
/// the same symbol/keyword. Non-readable output (`print`/`str`) always emits the raw
/// name — bars are a *reader* round-trip device, not for human display.
fn write_symbol(out: &mut String, name: &str, readable: bool, keyword: bool) {
    if readable && symbol_needs_bars(name, keyword) {
        out.push('|');
        for c in name.chars() {
            if c == '|' || c == '\\' {
                out.push('\\'); // the two chars the bar reader treats as escapes
            }
            out.push(c);
        }
        out.push('|');
    } else {
        out.push_str(name);
    }
}

/// Would the bare `name` fail to read back as this same symbol/keyword? Symbols and
/// keywords built from arbitrary strings (`(symbol "a b")`, `(keyword "")`) can hold
/// spellings that aren't a clean atom token — those need `|…|` bars to round-trip.
pub fn symbol_needs_bars(name: &str, keyword: bool) -> bool {
    use crate::syntax::atom::{AtomKind, classify, is_delimiter};
    // Not a single clean atom token: empty, or holding whitespace / a delimiter / the
    // bar-escape chars themselves / a control character (which the reader refuses in a
    // bare token but reads verbatim between bars). True for both symbols and keywords.
    if name.is_empty()
        || name
            .chars()
            .any(|c| is_delimiter(c) || c == '|' || c == '\\' || c.is_control())
    {
        return true;
    }
    if keyword {
        // `:name` reads back as this keyword for any clean, non-empty name — even a
        // numeric-looking one (`:123` is the keyword `123`) — so nothing more to check.
        false
    } else {
        // `#` and `^` are reader **dispatch** characters at form start (`#{…}`/`#b"…"`,
        // and `^x` → `(%pin x)`) while correctly *not* being atom delimiters — so a
        // bare `^foo` re-reads as `(%pin foo)`, a silently different value, and `#foo`
        // as a hard parse error. Only the FIRST character dispatches: `a^b` and `a#`
        // are ordinary atom text and stay bare. (Keywords are exempt — `:^foo`/`:#foo`
        // read back as themselves, since the `:` has already committed the token.)
        //
        // A lone `.` is the dotted-pair separator inside a list, not a symbol; and a
        // name that classifies as anything but a plain symbol (a number, `nil`/`true`/
        // `false`/`inf`/`nan`, or a `:`-led keyword) would read back as that value.
        name == "."
            || matches!(name.chars().next(), Some('#') | Some('^'))
            || !matches!(classify(name), AtomKind::Symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::reader;

    /// Readable output of a string must re-read to the same value: every
    /// control char the reader can decode must print in a form the reader
    /// understands. This guards the reader/printer round-trip for ESC, NUL,
    /// and other C0 controls.
    #[test]
    fn readable_strings_round_trip_through_the_reader() {
        let mut heap = Heap::new();
        let original = "a\u{1b}b\0c\n\t\r\u{7}\u{1f}\u{7f}\"\\d";
        let v = heap.alloc_string(original);
        let printed = print(&heap, v);
        // Re-read the printed text and confirm we recover the same string.
        let back = reader::read_one(&mut heap, &printed).expect("re-reads");
        match back.unpack() {
            ValueRef::Str(id) => assert_eq!(heap.string(id), original),
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn control_chars_use_named_or_numeric_escapes() {
        let mut heap = Heap::new();
        let cases = [
            ("\u{1b}", "\"\\e\""),     // ESC → \e
            ("\0", "\"\\0\""),         // NUL → \0
            ("\u{7}", "\"\\u{7}\""),   // BEL → numeric (no named escape)
            ("\u{7f}", "\"\\u{7f}\""), // DEL → numeric
            ("\n", "\"\\n\""),         // existing named escapes unchanged
        ];
        for (raw, want) in cases {
            let v = heap.alloc_string(raw);
            assert_eq!(print(&heap, v), want, "printing {raw:?}");
        }
    }
}
