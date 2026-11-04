//! `%json-encode-plain`: `json/encode` natively, for the values its built-in rules cover.
//! The encoder is Brood (`std/json.blsp`), and on plain data it spent most of its time
//! interpreting itself — a 400-op game frame took 14 ms of a 15 ms frame in the
//! interpreter (the wasm build has no JIT). This walks the same rules in Rust and answers
//! nil for anything else — a record, a value with a `JsonEncode` impl, a ratio or
//! decimal, a NaN, a lazy seq, a key that is not a string, keyword, symbol or int — so
//! `json/encode` falls back to the Brood path, which decides those exactly as before.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::InterruptPoll;

use super::args::arg;

/// Deeper than this, hand the value to the Brood encoder (which has its own stack story).
const MAX_DEPTH: usize = 512;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%json-encode-plain",
        Arity::exact(1),
        Sig::new(vec![any], string.union(nil_ty)),
        &["v"],
        "`v` as JSON text when it is plain data — nil, a bool, an int, a finite float, a string, a keyword or symbol (its name), a vector or proper list, or a map (not a record) keyed by strings, keywords, symbols or ints — exactly as `json/encode` writes it; nil for anything else, which `json/encode` then encodes (or refuses) its own way.",
        json_encode_plain,
    );
}

fn json_encode_plain(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut out = String::new();
    if encode(heap, arg(args, 0), &mut out, 0, &mut InterruptPoll::new())? {
        Ok(heap.alloc_string(&out))
    } else {
        Ok(Value::nil())
    }
}

/// Append `v`'s JSON to `out`; false when `v` holds anything the plain rules do not cover.
/// `poll` probes for an interrupt every few thousand values (ADR-491): a large document is
/// walked here in one native call.
fn encode(
    heap: &Heap,
    v: Value,
    out: &mut String,
    depth: usize,
    poll: &mut InterruptPoll,
) -> Result<bool, LispError> {
    poll.tick()?;
    if depth > MAX_DEPTH {
        return Ok(false);
    }
    match v {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) if f.is_finite() => out.push_str(&crate::syntax::printer::format_float(f)),
        Value::Str(id) => escape(heap.string(id), out),
        Value::Keyword(s) | Value::Sym(s) => escape(value::symbol_name_ref(s), out),
        Value::Vector(id) => {
            out.push('[');
            for (i, item) in heap.vector(id).iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if !encode(heap, *item, out, depth + 1, poll)? {
                    return Ok(false);
                }
            }
            out.push(']');
        }
        Value::Pair(_) => {
            out.push('[');
            let mut cur = v;
            let mut first = true;
            loop {
                match cur {
                    Value::Nil => break,
                    Value::Pair(p) => {
                        let (head, tail) = heap.pair(p);
                        if !first {
                            out.push(',');
                        }
                        first = false;
                        if !encode(heap, head, out, depth + 1, poll)? {
                            return Ok(false);
                        }
                        cur = tail;
                    }
                    // an improper tail, or a lazy one: the Brood encoder's to judge
                    _ => return Ok(false),
                }
            }
            out.push(']');
        }
        // A record (ADR-542) encodes as its fields, or by its own impl: the Brood encoder's.
        Value::Record { .. } => return Ok(false),
        Value::Map(id) => {
            out.push('{');
            for (i, (key, item)) in heap.map_entries(id).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match key {
                    Value::Str(s) => escape(heap.string(s), out),
                    Value::Keyword(s) | Value::Sym(s) => escape(value::symbol_name_ref(s), out),
                    Value::Int(n) => escape(&n.to_string(), out),
                    _ => return Ok(false),
                }
                out.push(':');
                if !encode(heap, item, out, depth + 1, poll)? {
                    return Ok(false);
                }
            }
            out.push('}');
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `s` as a JSON string: `json.blsp`'s `enc-string` — the named escapes, every other
/// control character as a lowercase `\u00xx`, everything else as itself.
fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
