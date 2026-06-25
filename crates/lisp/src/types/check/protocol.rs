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

/// The known interfaces (`defprotocol` *and* `defbehaviour`), keyed by name. Starts
/// from the runtime `*protocols*` registry — imported interfaces, populated by Pass
/// 1's `(:use …)` evals, so a behaviour declared in another module (the common case:
/// a framework declares it, an app implements it) is known — then the file's own
/// declarations fill in / override.
pub(super) fn collect(heap: &Heap, forms: &[Value]) -> HashMap<String, Protocol> {
    let mut ifaces = from_registry(heap);
    for &form in forms {
        if let Some((name, proto)) = parse_protocol(heap, form) {
            ifaces.insert(name, proto);
        }
    }
    ifaces
}

/// Read the runtime `*protocols*` registry (a name-symbol → raw-op-specs map that
/// `defprotocol`/`defbehaviour` populate). Empty when the registry isn't loaded.
fn from_registry(heap: &Heap) -> HashMap<String, Protocol> {
    let mut out = HashMap::new();
    let Some(Value::Map(id)) = heap.env_get(heap.global(), value::intern("*protocols*")) else {
        return out;
    };
    for (key, specs) in heap.map_entries(id) {
        let Some(name) = sym_name(key) else {
            continue;
        };
        let ops = list_items(heap, specs)
            .unwrap_or_default()
            .iter()
            .filter_map(|&op| parse_op(heap, op))
            .collect();
        out.insert(name, Protocol { ops });
    }
    out
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
        let pos = heap.form_pos_only(form);
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

/// `(defprotocol Name doc? (op [args] …) …)` or `(defbehaviour Name …)` → (name,
/// model), else `None`. Protocols and behaviours share the op-spec shape; they
/// differ only in *who* implements them (a `defimpl` vs a module's own functions).
fn parse_protocol(heap: &Heap, form: Value) -> Option<(String, Protocol)> {
    let items = list_items(heap, form)?;
    if !head_is(&items, "defprotocol") && !head_is(&items, "defbehaviour") {
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

/// The name of a symbol `Value` (and of a keyword, whose inner is a symbol), or
/// `None` otherwise.
fn sym_name(v: Value) -> Option<String> {
    match v {
        Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
        _ => None,
    }
}

// ---- behaviour conformance: `(:implements Name)` on a module ----------------

/// Check every module that declares `(:implements Name)` against the named interface
/// (`defbehaviour`/`defprotocol`): the module must *define* each declared op as a
/// function at the declared arity. Providers are read from the **expanded** tree, so
/// functions a macro generates (a `deflive` view's `mount`/`render`/…) count.
pub(super) fn check_behaviours(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ifaces: &HashMap<String, Protocol>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let claims = implements_claims(heap, forms);
    if claims.is_empty() {
        return;
    }
    let provided = defn_arities(heap, expanded);
    for (bname, pos) in claims {
        let Some(iface) = ifaces.get(&bname) else {
            // Unknown behaviour — declared in a module this file doesn't import, or
            // not yet defined. Stay quiet rather than false-flag.
            continue;
        };
        for op in &iface.ops {
            match provided.get(&op.name) {
                None => out.push((
                    pos,
                    format!(
                        "behaviour {}: this module is missing `{}` ({} arg(s))",
                        bname, op.name, op.arity
                    ),
                )),
                Some(&Some(arity)) if arity != op.arity => out.push((
                    pos,
                    format!(
                        "behaviour {}: `{}` takes {} arg(s), the behaviour needs {}",
                        bname, op.name, arity, op.arity
                    ),
                )),
                _ => {}
            }
        }
    }
}

/// The behaviour names a file's `(defmodule … (:implements Name) …)` header claims,
/// each with the module form's position for the diagnostic.
fn implements_claims(heap: &Heap, forms: &[Value]) -> Vec<(String, Option<Pos>)> {
    let mut out = Vec::new();
    for &form in forms {
        let Some(items) = list_items(heap, form) else {
            continue;
        };
        if !head_is(&items, "defmodule") {
            continue;
        }
        let pos = heap.form_pos_only(form);
        for &clause in items.get(2..).unwrap_or(&[]) {
            let Some(citems) = list_items(heap, clause) else {
                continue;
            };
            let is_implements =
                matches!(citems.first(), Some(&Value::Keyword(k)) if value::symbol_is(k, "implements"));
            if is_implements {
                if let Some(name) = citems.get(1).and_then(|&v| sym_name(v)) {
                    out.push((name, pos));
                }
            }
        }
    }
    out
}

/// Every function defined in the expanded tree → its arity, as `name → arity`. The
/// name is the *bare* last segment (`mod/render` → `render`) so it matches a
/// behaviour's bare op names; the arity is `None` for a variadic or multi-arity fn
/// (present, but no single arity to pin). Mirrors `walk::collect_def_names`'s
/// recursion (a `def` can nest inside a macro's `do`).
fn defn_arities(heap: &Heap, forms: &[Value]) -> HashMap<String, Option<usize>> {
    let mut out = HashMap::new();
    for &form in forms {
        collect_arity(heap, form, &mut out);
    }
    out
}

fn collect_arity(heap: &Heap, form: Value, out: &mut HashMap<String, Option<usize>>) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    if value::symbol_is(head, "quote") || value::symbol_is(head, "quasiquote") {
        return;
    }
    // `defn` has expanded to `(def name (fn …))` by now; `defmacro` stays itself.
    if value::symbol_is(head, "def") {
        if let Some(&Value::Sym(name)) = items.get(1) {
            let arity = items.get(2).and_then(|&v| fn_arity(heap, v));
            out.insert(bare_name(name), arity);
        }
    }
    for &item in items.get(1..).unwrap_or(&[]) {
        collect_arity(heap, item, out);
    }
}

/// The fixed arity of a `(fn …)`/`(lambda …)` value form, or `None` for a non-`fn`,
/// a variadic (`&` rest), or a multi-arity fn.
fn fn_arity(heap: &Heap, v: Value) -> Option<usize> {
    let items = list_items(heap, v)?;
    let is_fn = matches!(items.first(),
        Some(&Value::Sym(s)) if value::symbol_is(s, "fn") || value::symbol_is(s, "lambda"));
    if !is_fn {
        return None;
    }
    // After `fn`: the param list (single-arity), skipping a docstring.
    let rest = items.get(1..)?;
    let rest = match rest.first() {
        Some(Value::Str(_)) if rest.len() > 1 => &rest[1..],
        _ => rest,
    };
    let params = *rest.first()?;
    // Multi-arity: the "param list" is really a clause `((a) body…)` whose head is
    // itself a param list/vector → can't pin one arity.
    if let Some(pitems) = list_items(heap, params) {
        if matches!(pitems.first(), Some(Value::Pair(_)) | Some(Value::Vector(_))) {
            return None;
        }
    }
    param_count(heap, params)
}

/// The number of fixed parameters in a param list/vector, or `None` if it's variadic.
fn param_count(heap: &Heap, params: Value) -> Option<usize> {
    let items = match params {
        Value::Nil => return Some(0),
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Pair(_) => list_items(heap, params)?,
        _ => return None,
    };
    if items.iter().any(|&p| is_rest_marker(p)) {
        return None;
    }
    Some(items.len())
}

fn is_rest_marker(v: Value) -> bool {
    matches!(v, Value::Sym(s) if value::symbol_is(s, "&") || value::symbol_is(s, "&rest"))
}

/// A symbol's bare name — its last `/`-segment (`mod/render` → `render`).
fn bare_name(name: value::Symbol) -> String {
    let full = value::symbol_name(name);
    full.rsplit('/').next().unwrap_or(&full).to_string()
}
