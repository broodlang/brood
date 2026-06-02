//! Renders a [`Value`] to text. Needs the [`Heap`] to read heap objects.
//!
//! - [`print`] is *readable* (strings quoted/escaped) — what the REPL shows.
//! - [`display`] is *human* (strings raw) — what `print`/`str` use.

use crate::core::heap::Heap;
use crate::core::value::{symbol_name_ref, Value};

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
    match v {
        Value::Nil => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        // A bignum prints as its decimal string, exactly like an `Int`.
        Value::BigInt(id) => out.push_str(&heap.bigint(id).to_string()),
        Value::Float(f) => out.push_str(&format_float(f)),
        Value::Sym(s) => out.push_str(symbol_name_ref(s)),
        Value::Keyword(s) => {
            out.push(':');
            out.push_str(symbol_name_ref(s));
        }
        Value::Str(id) => {
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
                        _ => out.push(c),
                    }
                }
                out.push('"');
            } else {
                out.push_str(s);
            }
        }
        Value::Pair(_) => write_list(out, heap, v, readable, depth),
        Value::Vector(id) => {
            out.push('[');
            for (i, &item) in heap.vector(id).iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_value(out, heap, item, readable, depth + 1);
            }
            out.push(']');
        }
        Value::Map(id) => {
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
        Value::Fn(id) => {
            out.push_str("#<fn");
            if let Some(name) = heap.closure(id).name {
                out.push(' ');
                out.push_str(symbol_name_ref(name));
            }
            out.push('>');
        }
        Value::Macro(id) => {
            out.push_str("#<macro");
            if let Some(name) = heap.closure(id).name {
                out.push(' ');
                out.push_str(symbol_name_ref(name));
            }
            out.push('>');
        }
        Value::Native(id) => {
            out.push_str("#<native ");
            out.push_str(&heap.native(id).name);
            out.push('>');
        }
        Value::Ref(n) => {
            out.push_str("#<ref ");
            out.push_str(&n.to_string());
            out.push('>');
        }
        Value::Pid { node, id } => {
            out.push_str("#<pid ");
            out.push_str(symbol_name_ref(node));
            out.push('/');
            out.push_str(&id.to_string());
            out.push('>');
        }
        Value::Rope(id) => {
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
        Value::Socket(id) => {
            // A socket is a live OS resource with no readable literal.
            out.push_str("#<socket ");
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
        match cur {
            Value::Pair(p) => {
                if !first {
                    out.push(' ');
                }
                first = false;
                let (head, tail) = heap.pair(p);
                write_value(out, heap, head, readable, depth + 1);
                cur = tail;
            }
            Value::Nil => break,
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
