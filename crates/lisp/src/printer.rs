//! Renders a [`Value`] to text. Needs the [`Heap`] to read heap objects.
//!
//! - [`print`] is *readable* (strings quoted/escaped) — what the REPL shows.
//! - [`display`] is *human* (strings raw) — what `print`/`str` use.

use crate::heap::Heap;
use crate::value::{symbol_name, Value};

pub fn print(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    write_value(&mut out, heap, v, true);
    out
}

pub fn display(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    write_value(&mut out, heap, v, false);
    out
}

fn write_value(out: &mut String, heap: &Heap, v: Value, readable: bool) {
    match v {
        Value::Nil => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => out.push_str(&format_float(f)),
        Value::Sym(s) => out.push_str(&symbol_name(s)),
        Value::Keyword(s) => {
            out.push(':');
            out.push_str(&symbol_name(s));
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
        Value::Pair(_) => write_list(out, heap, v, readable),
        Value::Vector(id) => {
            out.push('[');
            for (i, &item) in heap.vector(id).iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_value(out, heap, item, readable);
            }
            out.push(']');
        }
        Value::Fn(id) => {
            out.push_str("#<fn");
            if let Some(name) = heap.closure(id).name {
                out.push(' ');
                out.push_str(&symbol_name(name));
            }
            out.push('>');
        }
        Value::Macro(id) => {
            out.push_str("#<macro");
            if let Some(name) = heap.closure(id).name {
                out.push(' ');
                out.push_str(&symbol_name(name));
            }
            out.push('>');
        }
        Value::Native(id) => {
            out.push_str("#<native ");
            out.push_str(&heap.native(id).name);
            out.push('>');
        }
    }
}

fn write_list(out: &mut String, heap: &Heap, v: Value, readable: bool) {
    out.push('(');
    let mut cur = v;
    let mut first = true;
    loop {
        match cur {
            Value::Pair(p) => {
                if !first {
                    out.push(' ');
                }
                first = false;
                let (head, tail) = heap.pair(p);
                write_value(out, heap, head, readable);
                cur = tail;
            }
            Value::Nil => break,
            other => {
                out.push_str(" . ");
                write_value(out, heap, other, readable);
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
        return if f > 0.0 { "inf".to_string() } else { "-inf".to_string() };
    }
    let s = format!("{}", f);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{}.0", s)
    }
}
