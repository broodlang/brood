//! Renders a [`Value`] to text. Needs the [`Heap`] to read heap objects.
//!
//! - [`print`] is *readable* (strings quoted/escaped) — what the REPL shows.
//! - [`display`] is *human* (strings raw) — what `print`/`str` use.

use crate::core::heap::Heap;
use crate::core::value::{symbol_name_ref, Value, ValueRef};

/// Maximum nesting the printer will descend into. Past this we emit `…`
/// rather than recursing — a printed REPL value should never be the thing
/// that overflows the native Rust stack. (The reader caps inputs at the
/// same depth, but a closure built by `cons`-ing in a loop can produce a
/// value deeper than any reader could parse.)
const MAX_DEPTH: u32 = 256;

pub fn print(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    write_value(&mut out, heap, v, true, 0);
    out
}

pub fn display(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    write_value(&mut out, heap, v, false, 0);
    out
}

fn write_value(out: &mut String, heap: &Heap, v: Value, readable: bool, depth: u32) {
    if depth >= MAX_DEPTH {
        out.push('…');
        return;
    }
    match v.unpack() {
        ValueRef::Nil => out.push_str("nil"),
        ValueRef::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        ValueRef::Int(n) => out.push_str(&n.to_string()),
        // A bignum prints as its decimal string, exactly like an `Int`.
        ValueRef::BigInt(id) => out.push_str(&heap.bigint(id).to_string()),
        // A bitset prints as an opaque handle — its bytes are raw, not text.
        ValueRef::Bitset(id) => out.push_str(&format!("#<bitset {} bytes>", heap.bitset(id).len())),
        ValueRef::Float(f) => out.push_str(&format_float(f)),
        ValueRef::Sym(s) => out.push_str(symbol_name_ref(s)),
        ValueRef::Keyword(s) => {
            out.push(':');
            out.push_str(symbol_name_ref(s));
        }
        ValueRef::Str(id) => {
            let s = heap.string(id);
            if readable {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        '\r' => out.push_str("\\r"),
                        // Match the reader's own spellings for ESC and NUL
                        // (scanner::scan_string_body decodes `\e`/`\0`); any
                        // *other* C0 control char (and DEL) has no named
                        // escape, so emit the reader's `\u{H..H}` form rather
                        // than a raw byte. The result re-reads to the same
                        // string — readable output must round-trip.
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
            } else {
                out.push_str(s);
            }
        }
        ValueRef::Pair(_) => write_list(out, heap, v, readable, depth),
        // A range prints as the list it stands in for: `(0 1 2 3 4)`.
        ValueRef::Range(id) => {
            out.push('(');
            let (lo, hi, step) = heap.range_parts(id);
            let mut i = lo;
            let mut first = true;
            while if step > 0 { i < hi } else { i > hi } {
                if !first {
                    out.push(' ');
                }
                first = false;
                write_value(out, heap, Value::int(i), readable, depth + 1);
                i += step;
            }
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
            other => {
                out.push_str(" . ");
                write_value(out, heap, other, readable, depth + 1);
                break;
            }
        }
    }
    out.push(')');
}

fn format_float(f: f64) -> String {
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
    let s = format!("{}", f);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{}.0", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::reader;

    /// Readable output of a string must re-read to the same value: every
    /// control char the reader can decode must print in a form the reader
    /// understands. This guards the reader/printer round-trip for ESC, NUL,
    /// and other C0 controls (kernel audit: they used to print as raw bytes).
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
