//! The printer: renders a [`Value`] back to text (the "print" of the REPL).
//!
//! Two modes:
//! - [`print`] is *readable*: strings are quoted/escaped so the output could be
//!   read back in. This is what the REPL shows.
//! - [`display`] is *human*: strings are raw. This is what `print`/`str` use.

use crate::value::{self, Value};

pub fn print(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v, true);
    out
}

pub fn display(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v, false);
    out
}

fn write_value(out: &mut String, v: &Value, readable: bool) {
    match v {
        Value::Nil => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => out.push_str(&format_float(*f)),
        Value::Str(s) => {
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
        Value::Sym(s) => out.push_str(&value::symbol_name(*s)),
        Value::Keyword(s) => {
            out.push(':');
            out.push_str(&value::symbol_name(*s));
        }
        Value::Pair(_) => write_list(out, v, readable),
        Value::Vector(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_value(out, item, readable);
            }
            out.push(']');
        }
        Value::Fn(c) => {
            out.push_str("#<fn");
            if let Some(name) = c.name {
                out.push(' ');
                out.push_str(&value::symbol_name(name));
            }
            out.push('>');
        }
        Value::Macro(c) => {
            out.push_str("#<macro");
            if let Some(name) = c.name {
                out.push(' ');
                out.push_str(&value::symbol_name(name));
            }
            out.push('>');
        }
        Value::Native(nf) => {
            out.push_str("#<native ");
            out.push_str(&nf.name);
            out.push('>');
        }
    }
}

fn write_list(out: &mut String, v: &Value, readable: bool) {
    out.push('(');
    let mut cur = v.clone();
    let mut first = true;
    loop {
        match cur {
            Value::Pair(p) => {
                if !first {
                    out.push(' ');
                }
                first = false;
                write_value(out, &p.0, readable);
                cur = p.1.clone();
            }
            Value::Nil => break,
            other => {
                // An improper (dotted) list: (a b . c)
                out.push_str(" . ");
                write_value(out, &other, readable);
                break;
            }
        }
    }
    out.push(')');
}

/// Format a float so it always reads back as a float (e.g. `3` -> `3.0`).
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
