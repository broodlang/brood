//! Protocol conformance: model `(defprotocol …)` and check each `(defimpl …)`
//! against it — a diagnostic per missing op, arity mismatch, or method the
//! protocol doesn't declare.
//!
//! Read from the **un-expanded** forms. `defprotocol`/`defimpl` lower to `defn`s
//! plus registry calls, so the protocol structure only survives before macro
//! expansion — the same reason `sig` (see `annot`) and the hygiene lint read the
//! un-expanded tree.
//!
//! The checker keys off the surface syntax (`defprotocol`/`defimpl` as bare heads,
//! op specs `(op [args] …)`); it doesn't need the macros' definitions, so this is a
//! pure static analysis independent of where the macros live (the std/Hatch prototype).

use std::collections::HashMap;

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::Pos;

use super::walk::list_items;

/// One declared op: its name and arity (its `[args]` count).
struct Op {
    name: String,
    arity: usize,
}

/// A protocol's declared ops.
pub(super) struct Protocol {
    ops: Vec<Op>,
}

/// Build the protocol model from a file's un-expanded top-level forms. A duplicate
/// `defprotocol` of the same name keeps the first.
pub(super) fn collect(heap: &Heap, forms: &[Value]) -> HashMap<String, Protocol> {
    let mut protos = HashMap::new();
    for &form in forms {
        if let Some((name, proto)) = parse_protocol(heap, form) {
            protos.entry(name).or_insert(proto);
        }
    }
    protos
}

/// Check each `(defimpl Proto key method…)` against `protos`: a diagnostic per op
/// the impl omits, per op whose arity disagrees with the protocol, and per method
/// the protocol doesn't declare. A `defimpl` of an unknown protocol is left alone —
/// it may be declared in another file.
pub(super) fn check_impls(
    heap: &Heap,
    forms: &[Value],
    protos: &HashMap<String, Protocol>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    for &form in forms {
        let Some(items) = list_items(heap, form) else {
            continue;
        };
        if !head_is(&items, "defimpl") {
            continue;
        }
        let Some(pname) = items.get(1).and_then(|&v| sym_name(v)) else {
            continue;
        };
        let Some(proto) = protos.get(&pname) else {
            continue;
        };
        let pos = heap.form_pos(form);
        // `(defimpl Proto key method…)` — the methods are items[3..].
        let provided: HashMap<String, usize> = items
            .get(3..)
            .unwrap_or(&[])
            .iter()
            .filter_map(|&m| parse_op(heap, m).map(|o| (o.name, o.arity)))
            .collect();
        // Every declared op must be implemented, at the declared arity.
        for op in &proto.ops {
            match provided.get(&op.name) {
                None => out.push((
                    pos,
                    format!("protocol {}: impl is missing op `{}`", pname, op.name),
                )),
                Some(&arity) if arity != op.arity => out.push((
                    pos,
                    format!(
                        "protocol {}: op `{}` takes {} arg(s), this impl has {}",
                        pname, op.name, op.arity, arity
                    ),
                )),
                Some(_) => {}
            }
        }
        // A method the protocol never declared is almost always a typo.
        for name in provided.keys() {
            if !proto.ops.iter().any(|o| &o.name == name) {
                out.push((pos, format!("protocol {}: has no op `{}`", pname, name)));
            }
        }
    }
}

/// `(defprotocol Name doc? (op [args] …) …)` → (name, model), else `None`.
fn parse_protocol(heap: &Heap, form: Value) -> Option<(String, Protocol)> {
    let items = list_items(heap, form)?;
    if !head_is(&items, "defprotocol") {
        return None;
    }
    let pname = sym_name(*items.get(1)?)?;
    // The op specs are the remaining list items; a leading docstring (a string, not
    // a list) is skipped by `parse_op` returning `None`.
    let ops = items
        .get(2..)
        .unwrap_or(&[])
        .iter()
        .filter_map(|&op| parse_op(heap, op))
        .collect();
    Some((pname, Protocol { ops }))
}

/// Parse `(name [args] …)` → its op name + arity. Shared by protocol op specs and
/// `defimpl` methods. `None` for a non-list (e.g. a docstring) or a malformed spec.
fn parse_op(heap: &Heap, form: Value) -> Option<Op> {
    let items = list_items(heap, form)?;
    let name = sym_name(*items.first()?)?;
    let arity = match *items.get(1)? {
        Value::Vector(id) => heap.vector(id).len(),
        _ => return None,
    };
    Some(Op { name, arity })
}

/// True when `items`' head is the symbol `name`.
fn head_is(items: &[Value], name: &str) -> bool {
    matches!(items.first(), Some(&Value::Sym(s)) if value::symbol_is(s, name))
}

/// The name of a symbol `Value`, or `None` if it isn't a symbol.
fn sym_name(v: Value) -> Option<String> {
    match v {
        Value::Sym(s) => Some(value::symbol_name(s)),
        _ => None,
    }
}
