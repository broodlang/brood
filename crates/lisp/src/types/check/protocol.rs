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

use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::error::Pos;

use super::walk::list_items;

/// One declared op: its name, its fixed arity (params before any `&`), and whether it
/// is variadic (has a `&`-rest). A declared op spec is always fixed; an *impl method* may
/// be variadic, in which case it satisfies the op as long as it accepts the declared arity.
struct Op {
    name: String,
    arity: usize,
    variadic: bool,
    /// A **provided** ability op — its `defability` spec carries a default body (ADR-185),
    /// so `defability` registers it as a `:default` impl. An `impl` need not supply it (the
    /// default covers it), but may override it — so it is not *required*, yet is still a
    /// valid op name (an override isn't flagged as unknown). Always false for a protocol /
    /// behaviour op and for an impl method (they have no default-body notion).
    provided: bool,
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
    // Record the dependency: this file's warnings depend on the whole `*protocols*`
    // table (it accumulates `defprotocol`/`extend` across files, so its def-site
    // alone can't capture a later extension — the Phase-2 fingerprint hashes its
    // full content instead).
    super::deps::obs_protocols(heap);
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
        // `(impl Ability key method…)` — an ability impl (`defprotocol`/`defimpl` retired).
        if !head_is(&items, "impl") {
            continue;
        }
        let noun = "ability";
        let Some(pname) = items.get(1).and_then(|&v| sym_name(v)) else {
            continue;
        };
        let Some(proto) = protos.get(&pname) else {
            continue;
        };
        let pos = heap.form_pos_only(form);
        // `(defimpl Proto key method…)` — the methods are items[3..]. Keep each method's
        // fixed arity *and* whether it is variadic (a `&`-rest impl satisfies a fixed op as
        // long as it accepts the declared arity — the `defability` variadic-op path).
        let provided: HashMap<String, (usize, bool)> = items
            .get(3..)
            .unwrap_or(&[])
            .iter()
            .filter_map(|&m| parse_op(heap, m).map(|o| (o.name, (o.arity, o.variadic))))
            .collect();
        // Every declared *required* op must be implemented, at a compatible arity. A
        // provided op (ADR-185) is covered by its default when omitted, so its absence is
        // fine; when present it is an override and is still arity-checked below.
        for op in &proto.ops {
            match provided.get(&op.name) {
                None if op.provided => {}
                None => out.push((
                    pos,
                    format!("{} {}: impl is missing op `{}`", noun, pname, op.name),
                )),
                Some(&(arity, impl_variadic)) => {
                    // A variadic OP declares no single arity to pin — accept any impl (mirrors
                    // the behaviour checker's treatment of multi-arity). Otherwise a fixed impl
                    // must match exactly; a variadic impl must accept the declared arity.
                    let compatible = op.variadic
                        || if impl_variadic {
                            arity <= op.arity
                        } else {
                            arity == op.arity
                        };
                    if !compatible {
                        out.push((
                            pos,
                            format!(
                                "{} {}: op `{}` takes {} arg(s), this impl has {}",
                                noun, pname, op.name, op.arity, arity
                            ),
                        ));
                    }
                }
            }
        }
        // A method the interface never declared is almost always a typo.
        for name in provided.keys() {
            if !proto.ops.iter().any(|o| &o.name == name) {
                out.push((pos, format!("{} {}: has no op `{}`", noun, pname, name)));
            }
        }
    }
}

/// `(defprotocol Name doc? (op [args] …) …)` or `(defbehaviour Name …)` → (name,
/// model), else `None`. Protocols and behaviours share the op-spec shape; they
/// differ only in *who* implements them (a `defimpl` vs a module's own functions).
fn parse_protocol(heap: &Heap, form: Value) -> Option<(String, Protocol)> {
    let items = list_items(heap, form)?;
    if !head_is(&items, "defbehaviour") && !head_is(&items, "defability") {
        return None;
    }
    let pname = sym_name(*items.get(1)?)?;
    // A `defability` op may be *provided* (a default body, ADR-185) — such an op is not
    // required of an impl. Protocols/behaviours have no such notion. The op specs are the
    // remaining list items; a leading docstring (a string, not a list) is skipped by
    // `parse_op` returning `None`.
    let is_ability = head_is(&items, "defability");
    let ops = items
        .get(2..)
        .unwrap_or(&[])
        .iter()
        .filter_map(|&op| {
            parse_op(heap, op).map(|mut o| {
                o.provided = is_ability && spec_has_body(heap, op);
                o
            })
        })
        .collect();
    Some((pname, Protocol { ops }))
}

/// Parse `(name [args] …)` → its op name, fixed arity, and variadic flag. Shared by
/// protocol op specs and `impl` methods. The fixed arity counts params before a `&`-rest.
/// `None` for a non-list (e.g. a docstring) or a malformed spec.
fn parse_op(heap: &Heap, form: Value) -> Option<Op> {
    let items = list_items(heap, form)?;
    let name = sym_name(*items.first()?)?;
    let args = match *items.get(1)? {
        Value::Vector(id) => heap.vector(id).to_vec(),
        _ => return None,
    };
    let variadic = args.iter().any(|&a| is_rest_marker(a));
    // Fixed params are those before the `&` marker (a variadic op takes >= that many).
    let arity = args.iter().take_while(|&&a| !is_rest_marker(a)).count();
    Some(Op {
        name,
        arity,
        variadic,
        provided: false,
    })
}

/// True when op spec `spec` carries a default-implementation body — a form beyond the arg
/// vector and the optional `:-> RET`. Such a **provided** op (ADR-185) is registered as a
/// `:default` impl by `defability`, so it is satisfied for every id and is never demanded
/// of a sealed member. The `:->` arrow, when present, always sits at index 2 (right after
/// the arg vector), so the body — if any — starts at index 4, else at index 2.
fn spec_has_body(heap: &Heap, spec: Value) -> bool {
    let Some(items) = list_items(heap, spec) else {
        return false;
    };
    let body_start = match items.get(2) {
        Some(&Value::Keyword(k)) if value::symbol_is(k, "->") => 4,
        _ => 2,
    };
    items.len() > body_start
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
            let is_implements = matches!(citems.first(), Some(&Value::Keyword(k)) if value::symbol_is(k, "implements"));
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
        if matches!(
            pitems.first(),
            Some(Value::Pair(_)) | Some(Value::Vector(_))
        ) {
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

// ---- ability missing-impl at call sites (Slice 3) ---------------------------
//
// Warn on a call to an ability op whose FIRST argument has a *statically known*
// identity — a literal, or a direct `defrecord` constructor call — for which no
// impl (and no `:default`) is registered.
//
// Soundness (the checker's no-false-positives rule): an op fn is recognised only by
// its EXACT def symbol, fingerprinted by an `(…/impl-for (quote [A op]) …)` in its
// body — so a same-named non-ability function is never mistaken for one, and a
// cross-file op call (whose def isn't in this tree) is simply not checked. An id is
// taken only when certain. The impl set unions this file's own `register-impl` forms
// (not eval'd at check time) with the runtime `*impls*` registry (cross-file
// reachable impls), so an impl in either place suppresses the warning.

/// `(quote X)` → `Some(X)`.
fn unquote(heap: &Heap, v: Value) -> Option<Value> {
    let items = list_items(heap, v)?;
    if items.len() == 2 && head_is(&items, "quote") {
        Some(items[1])
    } else {
        None
    }
}

/// The last body form of `(fn ARGS BODY…)` / `(lambda …)`, or `None` for a non-fn.
fn fn_last_body(heap: &Heap, v: Value) -> Option<Value> {
    let items = list_items(heap, v)?;
    let is_fn = matches!(items.first(),
        Some(&Value::Sym(s)) if value::symbol_is(s, "fn") || value::symbol_is(s, "lambda"));
    if !is_fn {
        return None;
    }
    items.last().copied()
}

/// Search `form` for an ability op's dispatch call → `(ability, op)`. `defability` emits
/// `(%dispatch *impls* (quote [A op]) id)` (the op-key at index 2, ADR-172 §7's inline
/// cache); the older `(impl-for (quote [A op]) id)` carried it at index 1.
fn find_op_key(heap: &Heap, form: Value) -> Option<(String, String)> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let items = list_items(heap, form)?;
        if let Some(&Value::Sym(h)) = items.first() {
            let opkey_idx = match sym_name(Value::Sym(h)).as_deref() {
                Some("%dispatch") => Some(2),
                Some(kw::IMPL_FOR) => Some(1),
                _ => None,
            };
            if let Some(idx) = opkey_idx {
                if let Some(Value::Vector(vid)) = items.get(idx).and_then(|&v| unquote(heap, v)) {
                    let vec = heap.vector(vid);
                    if let (Some(&a), Some(&o)) = (vec.first(), vec.get(1)) {
                        if let (Some(an), Some(on)) = (sym_name(a), sym_name(o)) {
                            return Some((an, on));
                        }
                    }
                }
            }
        }
        items.iter().find_map(|&item| find_op_key(heap, item))
    })
}

/// The record id (`:__id__` keyword's name) a `defrecord` constructor body carries —
/// a map literal `{:__id__ :ID …}` (the current form) or a legacy `(hash-map :__id__ …)`.
fn record_ctor_id(heap: &Heap, form: Value) -> Option<String> {
    // map literal body — `{:__id__ :ID …}`
    if let Value::Map(mid) = form {
        return heap.map_entries(mid).into_iter().find_map(|(k, v)| {
            matches!(k, Value::Keyword(kk) if value::symbol_is(kk, "__id__"))
                .then(|| sym_name(v))
                .flatten()
        });
    }
    let items = list_items(heap, form)?;
    if !matches!(items.first(), Some(&Value::Sym(h)) if bare_name(h) == "hash-map") {
        return None;
    }
    // pairs after the head: find the value following the `:__id__` key
    let mut i = 1;
    while i + 1 < items.len() {
        if matches!(items[i], Value::Keyword(k) if value::symbol_is(k, "__id__")) {
            return sym_name(items[i + 1]);
        }
        i += 2;
    }
    None
}

/// Collect this file's ability op fns (`def sym → (ability, op)`) and record ctors
/// (`def sym → id name`). Recurses — a `def` can nest inside a macro's `do`.
///
/// `ambiguous` records any op-fn global symbol that two DIFFERENT abilities bind (two
/// abilities declaring the same op name, so the later `defn` clobbers the earlier's
/// generic function — the same collision `register-ability` warns on at load). Such a
/// symbol is removed from `op_fns` by `build_ability_info` so the static missing-impl
/// pass neither false-warns (attributing a call to the wrong ability) nor false-passes
/// (suppressing a real gap because the name resolves to the other ability). The runtime
/// and `register-ability`'s warning cover that collision instead.
fn collect_ability_defs(
    heap: &Heap,
    form: Value,
    op_fns: &mut HashMap<value::Symbol, (String, String)>,
    ctors: &mut HashMap<value::Symbol, String>,
    ambiguous: &mut HashSet<value::Symbol>,
    collisions: &mut Vec<(Option<Pos>, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, "quote") || value::symbol_is(head, "quasiquote") {
            return;
        }
        if value::symbol_is(head, "def") {
            if let (Some(&Value::Sym(name)), Some(&val)) = (items.get(1), items.get(2)) {
                if let Some(body) = fn_last_body(heap, val) {
                    if let Some(key) = find_op_key(heap, body) {
                        match op_fns.get(&name) {
                            // A second, DIFFERENT ability binds the same op-fn symbol in this
                            // module: the later `defn` clobbers the earlier ability's generic
                            // function. Op names must be unique within a module (a different
                            // module's same-named op binds a distinct `<module>/op` global and
                            // is fine). Mark ambiguous *and* record a diagnostic — the runtime
                            // `register-ability` warns too; `nest check` makes it ship-blocking.
                            Some(prev) if *prev != key => {
                                ambiguous.insert(name);
                                collisions.push((
                                    heap.form_pos_only(form),
                                    format!(
                                        "ability {}: op `{}` is already declared by ability {} \
                                         in this module — op names must be unique per module \
                                         (rename one)",
                                        key.0, key.1, prev.0
                                    ),
                                ));
                            }
                            _ => {
                                op_fns.insert(name, key);
                            }
                        }
                    }
                    if let Some(id) = record_ctor_id(heap, body) {
                        ctors.insert(name, id);
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_ability_defs(heap, item, op_fns, ctors, ambiguous, collisions);
        }
    })
}

/// Collect `(…/register-impl (quote A) (quote op) :ID …)` forms from this file.
fn collect_register_impls(
    heap: &Heap,
    form: Value,
    impls: &mut HashSet<(String, String, String)>,
    defaults: &mut HashSet<(String, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::REGISTER_IMPL) {
                let a = items
                    .get(1)
                    .and_then(|&v| unquote(heap, v))
                    .and_then(sym_name);
                let op = items
                    .get(2)
                    .and_then(|&v| unquote(heap, v))
                    .and_then(sym_name);
                let id = items.get(3).copied().and_then(sym_name);
                if let (Some(a), Some(op), Some(id)) = (a, op, id) {
                    if id == "default" {
                        defaults.insert((a, op));
                    } else {
                        impls.insert((a, op, id));
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_impls(heap, item, impls, defaults);
        }
    })
}

/// Collect `(derive-into (quote A) :id (quote (fields)) (current-ns))` forms → `(ability A,
/// id)` pairs. `defrecord`'s `:derives` emits these; the recipe runs at *load*, not at check
/// time, so the checker can't see the generated methods directly. Instead it treats a derived
/// id as implementing **every** op of the ability (ADR-185) — so a derived member satisfies
/// call-site missing-impl and `:sealed` exhaustiveness without running the recipe.
fn collect_derive_into(heap: &Heap, form: Value, out: &mut Vec<(String, String)>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::DERIVE_INTO) {
                let a = items
                    .get(1)
                    .and_then(|&v| unquote(heap, v))
                    .and_then(sym_name);
                let id = items.get(2).copied().and_then(sym_name);
                if let (Some(a), Some(id)) = (a, id) {
                    out.push((a, id));
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_derive_into(heap, item, out);
        }
    })
}

/// Union in the runtime `*impls*` registry — `[A op] → {id → …}` — so an impl
/// reachable through a required module (not present as a form in this file) counts.
fn read_impls_registry(
    heap: &Heap,
    impls: &mut HashSet<(String, String, String)>,
    defaults: &mut HashSet<(String, String)>,
) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*impls*")) else {
        return;
    };
    for (op_key, inner) in heap.map_entries(mid) {
        let Value::Vector(vid) = op_key else { continue };
        let vec = heap.vector(vid);
        let (Some(&a), Some(&o)) = (vec.first(), vec.get(1)) else {
            continue;
        };
        let (Some(a), Some(o)) = (sym_name(a), sym_name(o)) else {
            continue;
        };
        let Value::Map(inner_id) = inner else {
            continue;
        };
        for (idk, _) in heap.map_entries(inner_id) {
            if let Some(id) = sym_name(idk) {
                if id == "default" {
                    defaults.insert((a.clone(), o.clone()));
                } else {
                    impls.insert((a.clone(), o.clone(), id));
                }
            }
        }
    }
}

/// The occurrence-typing domain of a sealed op (ADR-190): the member-union type
/// `%{__id__: (:a | :b | …)}` an argument to op `op_sym` must have, or `None` when no sound
/// demand exists (read from the per-file table `annot::set_sealed_op_domains` installed).
pub(super) fn sealed_op_domain(op_sym: value::Symbol) -> Option<crate::types::Ty> {
    let full = value::symbol_name(op_sym);
    let op_last = full.rsplit('/').next().unwrap_or(&full);
    let members = super::annot::sealed_op_members(op_last)?;
    // The SAME denotation `annot::ability_type` gives the ability's name, deliberately: a
    // value a `(sig f (Shape -> …))` accepts must not be rejected by the `(area s)` inside
    // it. Records become one open `%{__id__: (:a | :b | …)}` shape, built-in kind members
    // (`:int`, `:float`, …) their own lattice points — see `annot::sealed_members_ty`.
    super::annot::sealed_members_ty(&members)
}

/// Build the per-file sealed-op occurrence-typing domains from `AbilityInfo` (ADR-190): op-name
/// → member ids, for each op declared by **exactly one** ability, that ability **sealed** (a
/// closed set, so late-bound impls can't widen it), with **no `:default`** impl (which would
/// accept any value). Installed via `annot::set_sealed_op_domains` — `AbilityInfo` sees this
/// file's abilities, which the heap registries don't during `--check`.
pub(super) fn build_sealed_op_domains(info: &AbilityInfo) -> HashMap<String, Vec<String>> {
    let mut op_count: HashMap<&String, usize> = HashMap::new();
    for ops in info.abilities.values() {
        for op in ops {
            *op_count.entry(op).or_default() += 1;
        }
    }
    let mut out = HashMap::new();
    for (ability, members) in &info.sealed {
        let Some(ops) = info.abilities.get(ability) else {
            continue;
        };
        for op in ops {
            if op_count.get(op) == Some(&1)
                && !info.defaults.contains(&(ability.clone(), op.clone()))
            {
                out.insert(op.clone(), members.clone());
            }
        }
    }
    out
}

/// The ability-name-as-a-type table for `annot` (ADR-181/186): every known ability name →
/// `Some(member ids)` if it is **sealed** (the qualified, closed member set → a finite union
/// of record shapes) or `None` if it is **open** (→ the permissive `any`). Unions the file's
/// own `register-ability`/`register-sealed` forms (expanded tree; sealed ids already
/// ns-qualified) with the runtime `*abilities*` + `*sealed*` registries
/// (imported abilities). A name absent from the result is not an ability.
pub(super) fn ability_type_table(
    heap: &Heap,
    expanded: &[Value],
) -> HashMap<String, Option<Vec<String>>> {
    // All ability names (their op names are irrelevant here — just the name set).
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    let mut ret_forms: HashMap<(String, String), Value> = HashMap::new();
    let mut op_params: HashMap<(String, String), Vec<Option<crate::types::Ty>>> = HashMap::new();
    let mut provided: HashSet<(String, String)> = HashSet::new(); // unused here; the collectors require it
    for &form in expanded {
        collect_register_ability(
            heap,
            form,
            &mut names,
            &mut ret_forms,
            &mut op_params,
            &mut provided,
        );
    }
    read_abilities_registry(
        heap,
        &mut names,
        &mut ret_forms,
        &mut op_params,
        &mut provided,
    );
    // Sealed abilities → their closed member set.
    let mut sealed: HashMap<String, Vec<String>> = HashMap::new();
    for &form in expanded {
        collect_register_sealed(heap, form, &mut sealed);
    }
    read_sealed_registry(heap, &mut sealed);
    // Every ability → Some(members) if sealed, else None (open). A sealed ability declared
    // without a `defability` op list still counts (fold it in).
    let mut table: HashMap<String, Option<Vec<String>>> = HashMap::new();
    for name in names.into_keys() {
        let sealed_members = sealed.get(&name).cloned();
        table.insert(name, sealed_members);
    }
    for (name, members) in sealed {
        table.entry(name).or_insert(Some(members));
    }
    table
}

/// The statically-known identity name of a call argument, or `None` if not certain.
fn arg_identity(heap: &Heap, arg: Value, ctors: &HashMap<value::Symbol, String>) -> Option<String> {
    match arg {
        Value::Int(_) | Value::BigInt(_) => Some("int".into()),
        Value::Float(_) => Some("float".into()),
        Value::Str(_) => Some("string".into()),
        Value::Keyword(_) => Some("keyword".into()),
        Value::Bool(_) => Some("bool".into()),
        Value::Map(_) => Some("map".into()),
        Value::Vector(_) => Some("vector".into()),
        // a direct constructor call `(circle …)` → the record's id
        Value::Pair(_) => {
            let items = list_items(heap, arg)?;
            match items.first() {
                Some(&Value::Sym(h)) => ctors.get(&h).cloned(),
                _ => None,
            }
        }
        // a variable, `nil`, a fn, … → not statically certain
        _ => None,
    }
}

/// Walk every call site; warn when an ability op is applied to a known-identity arg
/// with no impl and no `:default`.
#[allow(clippy::too_many_arguments)]
/// Precomputed ability facts for a file: which globals are op fns, which record
/// constructors exist, and which `[ability op id]` are covered (this file's forms +
/// the runtime registry). Shared by the syntactic post-pass and the inference hook in
/// `check_into` (via `Ctx::ability`).
pub(super) struct AbilityInfo {
    op_fns: HashMap<value::Symbol, (String, String)>,
    ctors: HashMap<value::Symbol, String>,
    impls: HashSet<(String, String, String)>,
    defaults: HashSet<(String, String)>,
    /// ability name → its declared op names (for the sealed-exhaustiveness check).
    abilities: HashMap<String, Vec<String>>,
    /// `(ability, op)` → its declared **return type** (the `:-> RET` tail of the op
    /// spec, parsed to a `Ty`). Populated from this file's `register-ability` forms and
    /// the runtime `*abilities*` registry, so a return declared in another module is
    /// visible here too. Drives call-site return-type inference (`op_ret_of`) and the
    /// impl-return check.
    op_ret: HashMap<(String, String), crate::types::Ty>,
    /// `(ability, op)` → its declared **parameter types**, one entry per position (the
    /// `(name T)` sibling of `:-> RET`, ADR-180). `None` at a position = untyped there (the
    /// common case — `self` is untyped, and most params are bare). Drives call-site argument
    /// checking (`op_params_of`) and precise impl-body param binding. Same two sources as
    /// `op_ret`, so a param type declared in another module is visible here too.
    op_params: HashMap<(String, String), Vec<Option<crate::types::Ty>>>,
    /// ability name → its SEALED member id names (closed set), if declared sealed.
    sealed: HashMap<String, Vec<String>>,
    /// ability name → its REQUIRED (super-)abilities (ADR-193): any id implementing this
    /// ability must also implement each of them. Drives `check_requires`.
    requires: HashMap<String, Vec<String>>,
    /// `(ability, op)` pairs that are **provided** — the op carries a default body in its
    /// `defability` spec (ADR-185), registered as a `:default` impl. A provided op is
    /// satisfied for every id, so `check_sealed` never demands it of a sealed member.
    provided: HashSet<(String, String)>,
    /// Same-module op-name collisions: two abilities in this file declaring the same op
    /// name (the second clobbers the first's generic fn). Emitted by `check_op_collisions`.
    collisions: Vec<(Option<Pos>, String)>,
}

impl AbilityInfo {
    pub(super) fn is_empty(&self) -> bool {
        self.op_fns.is_empty()
    }
    /// The `(ability, op)` an op-fn global symbol denotes, if any.
    pub(super) fn op_of(&self, sym: value::Symbol) -> Option<&(String, String)> {
        self.op_fns.get(&sym)
    }
    /// The declared return type of the ability op the global symbol `sym` denotes, if
    /// any — the bridge that flows an op's `:-> RET` into call-site inference.
    pub(super) fn op_ret_of(&self, sym: value::Symbol) -> Option<&crate::types::Ty> {
        let (a, op) = self.op_fns.get(&sym)?;
        self.op_ret.get(&(a.clone(), op.clone()))
    }
    /// The declared return type of `(ability, op)`, if any — the by-name path used by
    /// the impl-return check (a `register-impl` carries the ability + op names).
    pub(super) fn op_ret_by_name(&self, ability: &str, op: &str) -> Option<&crate::types::Ty> {
        self.op_ret.get(&(ability.to_string(), op.to_string()))
    }
    /// The declared per-position parameter types of the ability op `sym` denotes, if the op
    /// declares any — for call-site argument checking.
    pub(super) fn op_params_of(
        &self,
        sym: value::Symbol,
    ) -> Option<&Vec<Option<crate::types::Ty>>> {
        let (a, op) = self.op_fns.get(&sym)?;
        self.op_params.get(&(a.clone(), op.clone()))
    }
    /// The declared per-position parameter types of `(ability, op)` — the by-name path used
    /// by the impl-body param binding.
    pub(super) fn op_params_by_name(
        &self,
        ability: &str,
        op: &str,
    ) -> Option<&Vec<Option<crate::types::Ty>>> {
        self.op_params.get(&(ability.to_string(), op.to_string()))
    }
    /// True when neither an impl nor a `:default` covers `(ability, op, id)`.
    pub(super) fn missing(&self, ability: &str, op: &str, id: &str) -> bool {
        !self
            .defaults
            .contains(&(ability.to_string(), op.to_string()))
            && !self
                .impls
                .contains(&(ability.to_string(), op.to_string(), id.to_string()))
    }
}

/// Gather the ability facts for a file from its expanded tree + the runtime registry.
pub(super) fn build_ability_info(heap: &Heap, expanded: &[Value]) -> AbilityInfo {
    let mut op_fns = HashMap::new();
    let mut ctors = HashMap::new();
    let mut ambiguous = HashSet::new();
    let mut collisions = Vec::new();
    for &form in expanded {
        collect_ability_defs(
            heap,
            form,
            &mut op_fns,
            &mut ctors,
            &mut ambiguous,
            &mut collisions,
        );
    }
    // Drop op-fn symbols two different abilities bound (a same-name clobber): their
    // attribution is uncertain, so they're excluded from the static missing-impl pass.
    for sym in &ambiguous {
        op_fns.remove(sym);
    }
    let mut impls = HashSet::new();
    let mut defaults = HashSet::new();
    for &form in expanded {
        collect_register_impls(heap, form, &mut impls, &mut defaults);
    }
    read_impls_registry(heap, &mut impls, &mut defaults);
    let mut abilities = HashMap::new();
    let mut sealed = HashMap::new();
    let mut requires: HashMap<String, Vec<String>> = HashMap::new();
    // `(ability, op)` → its `:-> RET` return-type *form* (unparsed). Filled from this
    // file's `register-ability` forms and the runtime registry, then parsed to `Ty`.
    let mut ret_forms: HashMap<(String, String), Value> = HashMap::new();
    // `(ability, op)` → its per-position parameter types (parsed eagerly, unlike `ret_forms`,
    // since `spec_param_types` already produces `Ty`s).
    let mut op_params: HashMap<(String, String), Vec<Option<crate::types::Ty>>> = HashMap::new();
    let mut provided: HashSet<(String, String)> = HashSet::new();
    for &form in expanded {
        collect_register_ability(
            heap,
            form,
            &mut abilities,
            &mut ret_forms,
            &mut op_params,
            &mut provided,
        );
        collect_register_sealed(heap, form, &mut sealed);
        collect_register_ability_requires(heap, form, &mut requires);
    }
    read_abilities_registry(
        heap,
        &mut abilities,
        &mut ret_forms,
        &mut op_params,
        &mut provided,
    );
    read_sealed_registry(heap, &mut sealed);
    read_requires_registry(heap, &mut requires);
    // `:derives [A]` on a record (ADR-185) registers A's impl for that id at LOAD, not at
    // check time — so expand each `derive-into` form into the impls set here: a derived id
    // implements every op of the ability. This makes a derived member satisfy the call-site
    // and `:sealed` checks (which read `impls`) without running the recipe.
    let mut derives: Vec<(String, String)> = Vec::new();
    for &form in expanded {
        collect_derive_into(heap, form, &mut derives);
    }
    for (a, id) in &derives {
        if let Some(ops) = abilities.get(a) {
            for op in ops {
                impls.insert((a.clone(), op.clone(), id.clone()));
            }
        }
    }
    let op_ret = ret_forms
        .into_iter()
        .filter_map(|(k, form)| super::annot::parse_type(heap, form).map(|ty| (k, ty)))
        .collect();
    AbilityInfo {
        op_fns,
        ctors,
        impls,
        defaults,
        abilities,
        op_ret,
        op_params,
        sealed,
        requires,
        provided,
        collisions,
    }
}

/// The `:-> RET` return-type form of an op spec `(op [params] :-> RET)`, or `None` when
/// the spec declares no return type. The arrow token is the keyword `:->` (a keyword
/// named `->`); the return type is the form that follows it.
fn spec_ret_form(heap: &Heap, spec: Value) -> Option<Value> {
    let items = list_items(heap, spec)?;
    let arrow = items
        .iter()
        .position(|&v| matches!(v, Value::Keyword(k) if value::symbol_is(k, "->")))?;
    items.get(arrow + 1).copied()
}

/// The per-position parameter types of an op spec `(op [p0 (p1 T1) …] …)`: `Some(ty)` for a
/// typed `(name T)` entry, `None` for a bare symbol (or a `&`-rest marker). `None` (the
/// whole result) when the spec has no param vector or declares no type at all — so an op
/// with all-bare params contributes nothing to argument checking.
fn spec_param_types(heap: &Heap, spec: Value) -> Option<Vec<Option<crate::types::Ty>>> {
    let items = list_items(heap, spec)?;
    let Some(&Value::Vector(vid)) = items.get(1) else {
        return None;
    };
    let types: Vec<Option<crate::types::Ty>> = heap
        .vector(vid)
        .to_vec()
        .iter()
        .map(|&p| {
            // `(name T)` → parse T; a bare symbol / `&` → no type.
            list_items(heap, p)
                .and_then(|it| it.get(1).copied())
                .and_then(|t| super::annot::parse_type(heap, t))
        })
        .collect();
    // Skip an all-untyped op — nothing to check, and no need to store a vec of `None`s.
    types.iter().any(Option::is_some).then_some(types)
}

/// Same-module op-name collisions (two abilities in this file declaring the same op name).
/// One diagnostic per collision — advisory in the live image, ship-blocking under
/// `nest check` (which exits nonzero on any warning), so op names stay unique per module.
pub(super) fn check_op_collisions(info: &AbilityInfo, out: &mut Vec<(Option<Pos>, String)>) {
    out.extend(info.collisions.iter().cloned());
}

/// Collect `(…/register-ability (quote A) (quote OPS))` → ability A's op names, plus each
/// op's `:-> RET` return-type form into `rets` (keyed by `(ability, op)`).
fn collect_register_ability(
    heap: &Heap,
    form: Value,
    out: &mut HashMap<String, Vec<String>>,
    rets: &mut HashMap<(String, String), Value>,
    params: &mut HashMap<(String, String), Vec<Option<crate::types::Ty>>>,
    provided: &mut HashSet<(String, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::REGISTER_ABILITY) {
                if let (Some(a), Some(ops_form)) = (
                    items
                        .get(1)
                        .and_then(|&v| unquote(heap, v))
                        .and_then(sym_name),
                    items.get(2).and_then(|&v| unquote(heap, v)),
                ) {
                    let mut ops = Vec::new();
                    for &spec in list_items(heap, ops_form).unwrap_or_default().iter() {
                        let Some(op) = list_items(heap, spec)
                            .and_then(|it| it.first().copied())
                            .and_then(sym_name)
                        else {
                            continue;
                        };
                        if let Some(ret) = spec_ret_form(heap, spec) {
                            rets.entry((a.clone(), op.clone())).or_insert(ret);
                        }
                        if let Some(ptys) = spec_param_types(heap, spec) {
                            params.entry((a.clone(), op.clone())).or_insert(ptys);
                        }
                        if spec_has_body(heap, spec) {
                            provided.insert((a.clone(), op.clone()));
                        }
                        ops.push(op);
                    }
                    out.insert(a, ops);
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_ability(heap, item, out, rets, params, provided);
        }
    })
}

/// Collect `(…/register-sealed (quote A) (list :id …))` → ability A's sealed member ids.
fn collect_register_sealed(heap: &Heap, form: Value, out: &mut HashMap<String, Vec<String>>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::REGISTER_SEALED) {
                if let (Some(a), Some(&list_form)) = (
                    items
                        .get(1)
                        .and_then(|&v| unquote(heap, v))
                        .and_then(sym_name),
                    items.get(2),
                ) {
                    // the members are the args of the `(list :id …)` form
                    if let Some(litems) = list_items(heap, list_form) {
                        let members = litems
                            .get(1..)
                            .unwrap_or(&[])
                            .iter()
                            .filter_map(|&m| sym_name(m))
                            .collect();
                        out.insert(a, members);
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_sealed(heap, item, out);
        }
    })
}

/// Collect `(…/register-ability-requires (quote A) (list (quote R) …))` → ability A's required
/// (super-)abilities (ADR-193). The required names are emitted *quoted*, so each is unquoted.
fn collect_register_ability_requires(
    heap: &Heap,
    form: Value,
    out: &mut HashMap<String, Vec<String>>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::REGISTER_ABILITY_REQUIRES) {
                if let (Some(a), Some(&list_form)) = (
                    items
                        .get(1)
                        .and_then(|&v| unquote(heap, v))
                        .and_then(sym_name),
                    items.get(2),
                ) {
                    if let Some(litems) = list_items(heap, list_form) {
                        let reqs = litems
                            .get(1..)
                            .unwrap_or(&[])
                            .iter()
                            .filter_map(|&r| unquote(heap, r).and_then(sym_name))
                            .collect();
                        out.insert(a, reqs);
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_ability_requires(heap, item, out);
        }
    })
}

/// Union in the runtime `*ability-requires*` registry — name → its required abilities (bare
/// name symbols) — so a `:requires` declared in an imported module is visible here too.
fn read_requires_registry(heap: &Heap, out: &mut HashMap<String, Vec<String>>) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*ability-requires*"))
    else {
        return;
    };
    for (name, reqs) in heap.map_entries(mid) {
        if let Some(a) = sym_name(name) {
            let rs = list_items(heap, reqs)
                .unwrap_or_default()
                .iter()
                .filter_map(|&r| sym_name(r))
                .collect();
            out.entry(a).or_insert(rs);
        }
    }
}

/// Union in the runtime `*abilities*` registry — name → op specs — recording each
/// op's name and its `:-> RET` return-type form (the latter into `rets`).
fn read_abilities_registry(
    heap: &Heap,
    out: &mut HashMap<String, Vec<String>>,
    rets: &mut HashMap<(String, String), Value>,
    params: &mut HashMap<(String, String), Vec<Option<crate::types::Ty>>>,
    provided: &mut HashSet<(String, String)>,
) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*abilities*")) else {
        return;
    };
    for (name, specs) in heap.map_entries(mid) {
        if let Some(a) = sym_name(name) {
            let mut ops = Vec::new();
            for &spec in list_items(heap, specs).unwrap_or_default().iter() {
                let Some(op) = list_items(heap, spec)
                    .and_then(|it| it.first().copied())
                    .and_then(sym_name)
                else {
                    continue;
                };
                if let Some(ret) = spec_ret_form(heap, spec) {
                    rets.entry((a.clone(), op.clone())).or_insert(ret);
                }
                if let Some(ptys) = spec_param_types(heap, spec) {
                    params.entry((a.clone(), op.clone())).or_insert(ptys);
                }
                if spec_has_body(heap, spec) {
                    provided.insert((a.clone(), op.clone()));
                }
                ops.push(op);
            }
            out.entry(a).or_insert(ops);
        }
    }
}

/// The record ids `defrecord` has registered (`*record-ids*`, the same ground truth the
/// optimizer's constructor detection uses). Needed to disambiguate a sealed member: an
/// UNQUALIFIED member that also spells a built-in kind (`ratio`, `map`, `int`, …) could be
/// either, because a **root-namespace** `(defrecord ratio …)` registers under the bare
/// `:ratio` — the very same dispatch key the built-in kind uses. The language itself
/// conflates them (a real `1/2` dispatches to that record's impl), so the type has to pick
/// the same side the runtime does, and only the registry can say which exists.
pub(super) fn record_id_names(
    heap: &Heap,
    expanded: &[Value],
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    // Imported/already-loaded records: the runtime registry.
    if let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*record-ids*")) {
        for (id, _) in heap.map_entries(mid) {
            if let Some(name) = sym_name(id) {
                out.insert(name);
            }
        }
    }
    // THIS file's records, which the registry cannot know: `nest check` expands the file but
    // never evaluates it, so its `defrecord`s have not run. Same two-source union
    // `ability_type_table` needs for the same reason.
    for &form in expanded {
        collect_record_registers(heap, form, &mut out);
    }
    out
}

/// Walk for `(%record-register :ns/name (quote name))` — what `defrecord` expands to. The id
/// is a BARE keyword here, unlike `%register-sealed`'s quoted ability name, so it is read
/// directly rather than through `unquote`.
fn collect_record_registers(heap: &Heap, form: Value, out: &mut std::collections::HashSet<String>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some("%record-register") {
                if let Some(name) = items.get(1).copied().and_then(sym_name) {
                    out.insert(name);
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_record_registers(heap, item, out);
        }
    })
}

/// Union in the runtime `*sealed*` registry — name → member id keywords.
fn read_sealed_registry(heap: &Heap, out: &mut HashMap<String, Vec<String>>) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*sealed*")) else {
        return;
    };
    for (name, members) in heap.map_entries(mid) {
        if let Some(a) = sym_name(name) {
            let ids = list_items(heap, members)
                .unwrap_or_default()
                .iter()
                .filter_map(|&m| sym_name(m))
                .collect();
            out.entry(a).or_insert(ids);
        }
    }
}

/// Sealed-ability exhaustiveness: every sealed member must have a *direct* impl of every
/// declared **required** op (a `:default` doesn't count — sealing means each member is
/// handled explicitly). A **provided** op (one with a default body, ADR-185) is satisfied
/// by its default for every member, so it is skipped. One warning per missing
/// (ability, op, member).
/// Super-ability conformance (ADR-193): if ability `A` declares `:requires [R …]`, every id
/// that implements `A` must also implement each `R` (every op of `R`, directly or via a
/// `:default`/provided op). Advisory, like `check_sealed` — a declared conformance contract,
/// not a gate. An unknown required ability (not in the registry) is skipped (no false
/// positive). The implementor set is `A`'s sealed members plus any id with a direct `A` impl.
pub(super) fn check_requires(info: &AbilityInfo, out: &mut Vec<(Option<Pos>, String)>) {
    for (ability, reqs) in &info.requires {
        // Ids that implement `ability`: its sealed members (the intended set) + any id with a
        // direct impl of one of its ops.
        let mut implementors: std::collections::BTreeSet<&String> =
            std::collections::BTreeSet::new();
        if let Some(members) = info.sealed.get(ability) {
            implementors.extend(members);
        }
        for (a, _op, id) in &info.impls {
            if a == ability {
                implementors.insert(id);
            }
        }
        for req in reqs {
            let Some(req_ops) = info.abilities.get(req) else {
                continue; // required ability unknown here — can't check, no false positive
            };
            for id in &implementors {
                for op in req_ops {
                    // A provided op is satisfied by its default; a `:default` impl covers any id.
                    if info.provided.contains(&(req.clone(), op.clone()))
                        || info.defaults.contains(&(req.clone(), op.clone()))
                        || info
                            .impls
                            .contains(&(req.clone(), op.clone(), (*id).clone()))
                    {
                        continue;
                    }
                    out.push((
                        None,
                        format!(
                            "ability {} requires {}: :{} implements {} but has no impl of `{}` for {}",
                            ability, req, id, ability, op, req
                        ),
                    ));
                }
            }
        }
    }
}

pub(super) fn check_sealed(info: &AbilityInfo, out: &mut Vec<(Option<Pos>, String)>) {
    for (ability, members) in &info.sealed {
        let Some(ops) = info.abilities.get(ability) else {
            continue;
        };
        for op in ops {
            if info.provided.contains(&(ability.clone(), op.clone())) {
                continue;
            }
            for member in members {
                if !info
                    .impls
                    .contains(&(ability.clone(), op.clone(), member.clone()))
                {
                    out.push((
                        None,
                        format!(
                            "sealed ability {}: no impl of `{}` for :{}",
                            ability, op, member
                        ),
                    ));
                }
            }
        }
    }
}

/// The nominal id name of a record `Ty` — its `:__id__` field's `keyword_lit` singleton
/// — or `None` if `ty` isn't a record shape with a single known identity.
pub(super) fn ty_record_id(ty: &crate::types::Ty) -> Option<String> {
    let fields = ty.record_fields()?;
    let (id_ty, _) = fields.get(&value::intern("__id__"))?;
    let lits = id_ty.as_lit()?;
    let mut it = lits.iter();
    let only = it.next()?;
    if it.next().is_none() {
        Some(value::symbol_name(*only))
    } else {
        None
    }
}

/// Syntactic pass: warn on an ability op call whose FIRST argument has a statically
/// known identity from *syntax alone* — a literal or a direct `defrecord` constructor
/// call. The inference hook in `check_into` complements it for record-typed *variables*.
fn walk_ability_calls(
    heap: &Heap,
    form: Value,
    info: &AbilityInfo,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, "quote") || value::symbol_is(head, "quasiquote") {
            return;
        }
        if let Some((a, op)) = info.op_of(head) {
            if let Some(&arg) = items.get(1) {
                if let Some(id) = arg_identity(heap, arg, &info.ctors) {
                    if info.missing(a, op, &id) {
                        out.push((
                            heap.form_pos_only(form),
                            format!("ability {}: no impl of `{}` for :{}", a, op, id),
                        ));
                    }
                }
            }
        }
        for &item in &items {
            walk_ability_calls(heap, item, info, out);
        }
    })
}

pub(super) fn check_ability_calls(
    heap: &Heap,
    expanded: &[Value],
    info: &AbilityInfo,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if info.is_empty() {
        return;
    }
    for &form in expanded {
        walk_ability_calls(heap, form, info, out);
    }
}

/// The inference hook: at an ability op call on a *variable* whose inferred type is a
/// record, warn when no impl covers that record's identity. Called from `check_into`
/// (which has the local + global type context), complementing the syntactic pass.
pub(super) fn check_ability_call_inferred(
    info: &AbilityInfo,
    head: value::Symbol,
    arg_ty: &crate::types::Ty,
    pos: Option<Pos>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some((a, op)) = info.op_of(head) else {
        return;
    };
    if let Some(id) = ty_record_id(arg_ty) {
        if info.missing(a, op, &id) {
            out.push((
                pos,
                format!("ability {}: no impl of `{}` for :{}", a, op, id),
            ));
        }
    }
}

// ---- multimethod missing-method at call sites (ADR-179) ------------------------------
//
// The `defmulti` analogue of `check_ability_calls`: warn on a direct call to a multimethod
// generic whose FULL argument tuple has a statically-known identity — every arg a literal or
// a direct `defrecord` constructor call — for which no exact method and no `:default` is
// registered. Sound like the ability pass: a generic is recognised only by the exact def
// symbol whose body fingerprints as `(multi-resolve (quote NAME) …)`, so a same-named plain
// function is never mistaken for one, and a call whose generic is defined in another file is
// simply not checked. Only warns when *every* argument's identity is certain — one unknown
// arg defers. Methods union this file's `register-method` forms with the runtime `*methods*`
// registry (cross-file/reachable methods), so a method in either place suppresses the warning.

/// Precomputed multimethod facts for a file: which globals are `defmulti` generics, which
/// record constructors exist, and which identity-tuples (plus `:default`) each multimethod
/// covers (this file's `register-method` forms + the runtime `*methods*` registry).
pub(super) struct MultiInfo {
    /// generic-fn global symbol → the multimethod's name.
    generics: HashMap<value::Symbol, String>,
    /// record constructor symbol → its `:module/name` id-name (`(circle …)` → circle's id).
    ctors: HashMap<value::Symbol, String>,
    /// multimethod name → the set of exact identity-tuples it has a method for.
    methods: HashMap<String, HashSet<Vec<String>>>,
    /// multimethod names that have a `:default` catch-all method.
    defaults: HashSet<String>,
    /// id-names that are genuine `defrecord` identities (this file's ctors + the runtime
    /// `*record-ids*` registry). Lets the operator-sugar check tell a record operand from a
    /// plain number, so `(+ 1 2)` is never checked but `(+ (usd 1) 2.5)` is.
    record_ids: HashSet<String>,
    /// multimethod name → its declared `:-> RET` type. A multimethod's own body is the
    /// dispatch machinery, so its inferred type is opaque; the declaration is the only
    /// static handle on what a call yields — the role `AbilityInfo::op_ret` plays for an
    /// ability op, and sound for the same reason (`check_method_returns` verifies every
    /// method body against it, so it is a contract rather than a guess).
    rets: HashMap<String, crate::types::Ty>,
}

impl MultiInfo {
    fn is_empty(&self) -> bool {
        self.generics.is_empty()
    }
    /// The multimethod name a generic global symbol denotes, if any.
    pub(super) fn generic_of(&self, sym: value::Symbol) -> Option<&String> {
        self.generics.get(&sym)
    }
    /// The declared return type of the multimethod a generic symbol denotes — the call-site
    /// path, mirroring `AbilityInfo::op_ret_of`.
    pub(super) fn ret_of(&self, sym: value::Symbol) -> Option<&crate::types::Ty> {
        if let Some(ty) = self.generics.get(&sym).and_then(|m| self.rets.get(m)) {
            return Some(ty);
        }
        // Fall back to the symbol's own spelling. `generics` is built from the FILE's
        // expanded forms, so it only knows multimethods this file declares; a prelude or
        // cross-module one (`compare-to`, `num/add`) is absent and its call sites would go
        // untyped. A `defmulti` at root defines a global of exactly its own name, so for
        // those the symbol *is* the multimethod name.
        self.rets.get(&value::symbol_name(sym))
    }
    /// The declared return type of `mname` — the by-name path the method-return check uses
    /// (a `register-method` form carries the multimethod's name, not the generic symbol).
    pub(super) fn ret_by_name(&self, mname: &str) -> Option<&crate::types::Ty> {
        self.rets.get(mname)
    }
    /// True when neither an exact method nor a `:default` covers `mname`'s identity `tuple`.
    fn missing(&self, mname: &str, tuple: &[String]) -> bool {
        !self.defaults.contains(mname)
            && !self.methods.get(mname).is_some_and(|s| s.contains(tuple))
    }
}

/// The diagnostic for a multimethod call whose identity tuple has no method.
fn multi_missing_warning(mname: &str, tuple: &[String]) -> String {
    let key = tuple
        .iter()
        .map(|s| format!(":{}", s))
        .collect::<Vec<_>>()
        .join(" ");
    format!("multimethod {}: no method for [{}]", mname, key)
}

/// Search `form` for a multimethod generic's dispatch call → the multimethod name. `defmulti`
/// emits `(defn NAME (& args) (let (… (multi-resolve (quote NAME) key)) …))`, so the body
/// carries a `(multi-resolve (quote NAME) …)` — the fingerprint, mirroring `find_op_key`.
fn find_multi_name(heap: &Heap, form: Value) -> Option<String> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let items = list_items(heap, form)?;
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::MULTI_RESOLVE) {
                if let Some(name) = items
                    .get(1)
                    .and_then(|&v| unquote(heap, v))
                    .and_then(sym_name)
                {
                    return Some(name);
                }
            }
        }
        items.iter().find_map(|&item| find_multi_name(heap, item))
    })
}

/// Collect this file's multimethod generics (`def sym → name`) and record ctors
/// (`def sym → id`). Recurses — a `def` can nest inside a macro's `do`.
fn collect_multi_defs(
    heap: &Heap,
    form: Value,
    generics: &mut HashMap<value::Symbol, String>,
    ctors: &mut HashMap<value::Symbol, String>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, "quote") || value::symbol_is(head, "quasiquote") {
            return;
        }
        if value::symbol_is(head, "def") {
            if let (Some(&Value::Sym(name)), Some(&val)) = (items.get(1), items.get(2)) {
                if let Some(body) = fn_last_body(heap, val) {
                    if let Some(mname) = find_multi_name(heap, body) {
                        generics.insert(name, mname);
                    }
                    if let Some(id) = record_ctor_id(heap, body) {
                        ctors.insert(name, id);
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_multi_defs(heap, item, generics, ctors);
        }
    })
}

/// Collect `(register-method (quote NAME) KEY …)` forms — KEY is `(quote [id …])` for a
/// tuple method or a bare `:default` keyword for the catch-all.
fn collect_register_methods(
    heap: &Heap,
    form: Value,
    methods: &mut HashMap<String, HashSet<Vec<String>>>,
    defaults: &mut HashSet<String>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::REGISTER_METHOD) {
                if let Some(mname) = items
                    .get(1)
                    .and_then(|&v| unquote(heap, v))
                    .and_then(sym_name)
                {
                    match items.get(2).copied() {
                        Some(Value::Keyword(k)) if value::symbol_is(k, "default") => {
                            defaults.insert(mname);
                        }
                        Some(key) => {
                            if let Some(tuple) = key_tuple(heap, unquote(heap, key)) {
                                methods.entry(mname).or_default().insert(tuple);
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_methods(heap, item, methods, defaults);
        }
    })
}

/// A dispatch key vector `[id …]` → the tuple of id-name strings, or `None` if it isn't a
/// vector of symbols/keywords.
fn key_tuple(heap: &Heap, key: Option<Value>) -> Option<Vec<String>> {
    let Some(Value::Vector(vid)) = key else {
        return None;
    };
    heap.vector(vid).iter().map(|&e| sym_name(e)).collect()
}

/// Union in the runtime `*methods*` registry — `NAME → {tuple|:default → fn}` — so a method
/// reachable through a required module (not a form in this file) counts.
fn read_methods_registry(
    heap: &Heap,
    methods: &mut HashMap<String, HashSet<Vec<String>>>,
    defaults: &mut HashSet<String>,
) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*methods*")) else {
        return;
    };
    for (name_v, inner) in heap.map_entries(mid) {
        let Some(mname) = sym_name(name_v) else {
            continue;
        };
        let Value::Map(inner_id) = inner else {
            continue;
        };
        for (key, _fn) in heap.map_entries(inner_id) {
            match key {
                Value::Keyword(k) if value::symbol_is(k, "default") => {
                    defaults.insert(mname.clone());
                }
                Value::Vector(vid) => {
                    let tuple: Option<Vec<String>> =
                        heap.vector(vid).iter().map(|&e| sym_name(e)).collect();
                    if let Some(tuple) = tuple {
                        methods.entry(mname.clone()).or_default().insert(tuple);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Collect `(register-multi (quote NAME) ALGEBRA)` → NAME's closure-algebra name
/// (`commutative`/`antisymmetric`); a nil algebra is skipped.
fn collect_register_multi(
    heap: &Heap,
    form: Value,
    algebras: &mut HashMap<String, String>,
    rets: &mut HashMap<String, crate::types::Ty>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some(kw::REGISTER_MULTI) {
                if let Some(mname) = items
                    .get(1)
                    .and_then(|&v| unquote(heap, v))
                    .and_then(sym_name)
                {
                    if let Some(alg) = items.get(2).and_then(|&v| sym_name(v)) {
                        algebras.insert(mname.clone(), alg);
                    }
                    // arg 3 is the quoted `:-> RET` form, and it is ABSENT — not nil —
                    // when none was declared: `defmulti` emits a two-argument call in that
                    // case, so `get(3)` answers the question on its own. Treating a nil
                    // *value* as "undeclared" here would make `:-> nil` unenforceable, and
                    // `nil` is a legal return type (`parse_type` maps it to `Tag::Nil`,
                    // and both `sig` and `defability` accept it).
                    if let Some(form) = items.get(3).and_then(|&v| unquote(heap, v)) {
                        if let Some(ty) = super::annot::parse_type(heap, form) {
                            rets.insert(mname, ty);
                        }
                    }
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_multi(heap, item, algebras, rets);
        }
    })
}

/// Union in the runtime `*multi-algebra*` registry — NAME → algebra keyword (or nil).
fn read_multi_algebra_registry(heap: &Heap, algebras: &mut HashMap<String, String>) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*multi-algebra*"))
    else {
        return;
    };
    for (name_v, alg_v) in heap.map_entries(mid) {
        if let (Some(mname), Some(alg)) = (sym_name(name_v), sym_name(alg_v)) {
            algebras.entry(mname).or_insert(alg);
        }
    }
}

/// Union in the runtime `*multi-ret*` registry — multimethod name → its declared `:-> RET`
/// form — so a multimethod declared in another module is typed here too. The file's own
/// `%register-multi` forms win, matching how the algebra registry is merged.
fn read_multi_ret_registry(heap: &Heap, rets: &mut HashMap<String, crate::types::Ty>) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*multi-ret*")) else {
        return;
    };
    for (name_v, form) in heap.map_entries(mid) {
        let Some(mname) = sym_name(name_v) else {
            continue;
        };
        // A nil VALUE here is a declared `:-> nil`, not an absent declaration — `defmulti`
        // only registers the key when a return type was actually given (`%register-multi`
        // takes it as a variadic tail for exactly this reason), so absence is the key
        // missing, never a nil under it.
        if rets.contains_key(&mname) {
            continue;
        }
        if let Some(ty) = super::annot::parse_type(heap, form) {
            rets.insert(mname, ty);
        }
    }
}

/// Gather the multimethod facts for a file from its expanded tree + the runtime registry.
pub(super) fn build_multi_info(heap: &Heap, expanded: &[Value]) -> MultiInfo {
    let mut generics = HashMap::new();
    let mut ctors = HashMap::new();
    for &form in expanded {
        collect_multi_defs(heap, form, &mut generics, &mut ctors);
    }
    let mut methods = HashMap::new();
    let mut defaults = HashSet::new();
    for &form in expanded {
        collect_register_methods(heap, form, &mut methods, &mut defaults);
    }
    read_methods_registry(heap, &mut methods, &mut defaults);
    // Account for the closure mirror the runtime derives: a `:commutative`/`:antisymmetric`
    // multimethod's authored `[A B]` (A ≠ B) also covers `[B A]`. Without this, a call in the
    // mirror order (`(scale 3 money)` for a `[money :int]` method) would false-warn when the
    // file's own methods are read from forms (not yet in the runtime registry).
    let mut algebras = HashMap::new();
    let mut rets = HashMap::new();
    for &form in expanded {
        collect_register_multi(heap, form, &mut algebras, &mut rets);
    }
    read_multi_algebra_registry(heap, &mut algebras);
    read_multi_ret_registry(heap, &mut rets);
    for (mname, set) in methods.iter_mut() {
        if algebras.contains_key(mname) {
            let mirrors: Vec<Vec<String>> = set
                .iter()
                .filter(|t| t.len() == 2 && t[0] != t[1])
                .map(|t| vec![t[1].clone(), t[0].clone()])
                .collect();
            set.extend(mirrors);
        }
    }
    // Record ids: this file's own ctors' ids, plus the runtime `*record-ids*` registry.
    let mut record_ids: HashSet<String> = ctors.values().cloned().collect();
    read_record_ids_registry(heap, &mut record_ids);
    MultiInfo {
        generics,
        ctors,
        methods,
        defaults,
        record_ids,
        rets,
    }
}

/// Union in the runtime `*record-ids*` registry — id-keyword → record name (ADR-182) — so a
/// record type loaded from another module is known here too. Only the ids (keys) are needed.
fn read_record_ids_registry(heap: &Heap, out: &mut HashSet<String>) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("*record-ids*")) else {
        return;
    };
    for (id, _name) in heap.map_entries(mid) {
        if let Some(id) = sym_name(id) {
            out.insert(id);
        }
    }
}

/// The multimethod an arithmetic/comparison operator routes to on a record operand (ADR-179):
/// `+`/`-`/`*`/`/` → `num-*`, and `<`/`<=`/`>`/`>=` → `compare-to` (the antisymmetric mirror
/// makes the direction irrelevant). `None` for a non-operator head.
fn operator_multimethod(head: value::Symbol) -> Option<&'static str> {
    Some(if value::symbol_is(head, "+") {
        "num/add"
    } else if value::symbol_is(head, "-") {
        "num/sub"
    } else if value::symbol_is(head, "*") {
        "num/mul"
    } else if value::symbol_is(head, "/") {
        "num/div"
    } else if value::symbol_is(head, "<")
        || value::symbol_is(head, "<=")
        || value::symbol_is(head, ">")
        || value::symbol_is(head, ">=")
    {
        "compare-to"
    } else {
        return None;
    })
}

/// Operator-sugar coverage (ADR-179): a 2-arg `(+ a b)` / `(< a b)` etc. routes to a `Num`/`Ord`
/// multimethod when a record is an operand. Warn when both operand identities are known, at
/// least one is a record (so pure `(+ 1 2)` is never touched), and the routed multimethod has
/// no method for the pair. Only the 2-arg form — a variadic fold's intermediate type is unknown.
pub(super) fn check_operator_sugar(
    heap: &Heap,
    head: value::Symbol,
    items: &[Value],
    info: &MultiInfo,
    ctx: &super::ctx::Ctx,
    pos: Option<Pos>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if info.record_ids.is_empty() {
        return;
    }
    let Some(mname) = operator_multimethod(head) else {
        return;
    };
    let args = items.get(1..).unwrap_or(&[]);
    if args.len() != 2 {
        return;
    }
    let (Some(a), Some(b)) = (
        multi_arg_identity(heap, args[0], info, ctx),
        multi_arg_identity(heap, args[1], info, ctx),
    ) else {
        return;
    };
    // Only when a record is actually involved — else the kernel handles it (no `num-*` method
    // exists for `[int int]`, and warning there would be nonsense).
    if !info.record_ids.contains(&a) && !info.record_ids.contains(&b) {
        return;
    }
    let tuple = vec![a, b];
    if info.missing(mname, &tuple) {
        let key = tuple
            .iter()
            .map(|s| format!(":{}", s))
            .collect::<Vec<_>>()
            .join(" ");
        out.push((
            pos,
            format!(
                "{}: no `{}` method for [{}]",
                value::symbol_name(head),
                mname,
                key
            ),
        ));
    }
}

/// Walk every call site; warn when a multimethod generic is applied to a fully
/// statically-known argument tuple with no exact method and no `:default`.
fn walk_multi_calls(
    heap: &Heap,
    form: Value,
    info: &MultiInfo,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, "quote") || value::symbol_is(head, "quasiquote") {
            return;
        }
        if let Some(mname) = info.generics.get(&head) {
            let args = items.get(1..).unwrap_or(&[]);
            // Only judge a call every one of whose args has a certain identity — one unknown
            // arg (a variable, a call result) leaves the tuple unknown, so we defer.
            if !args.is_empty() {
                let tuple: Option<Vec<String>> = args
                    .iter()
                    .map(|&a| arg_identity(heap, a, &info.ctors))
                    .collect();
                if let Some(tuple) = tuple {
                    if info.missing(mname, &tuple) {
                        out.push((
                            heap.form_pos_only(form),
                            multi_missing_warning(mname, &tuple),
                        ));
                    }
                }
            }
        }
        for &item in &items {
            walk_multi_calls(heap, item, info, out);
        }
    })
}

pub(super) fn check_multi_calls(
    heap: &Heap,
    expanded: &[Value],
    info: &MultiInfo,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if info.is_empty() {
        return;
    }
    for &form in expanded {
        walk_multi_calls(heap, form, info, out);
    }
}

/// An argument's dispatch identity for a multimethod call: syntactic (a literal or a direct
/// `defrecord` ctor call) first, else the record id of its *inferred* type (a record-typed
/// variable). `None` when neither is certain — the tuple then stays unknown and the call is
/// not judged (sound: no false positive on an unknown operand).
pub(super) fn multi_arg_identity(
    heap: &Heap,
    arg: Value,
    info: &MultiInfo,
    ctx: &super::ctx::Ctx,
) -> Option<String> {
    if let Some(id) = arg_identity(heap, arg, &info.ctors) {
        return Some(id);
    }
    let ty = super::infer::expr_ty(heap, arg, ctx)?;
    ty_record_id(&ty)
}

/// The inference hook: at a `defmulti` generic call at least one of whose args is a *symbol*
/// (so the syntactic pass deferred — a symbol never resolves syntactically), resolve every
/// arg's identity syntactically-or-by-inferred-record-type and warn when the full tuple has no
/// method and no `:default`. Complements `walk_multi_calls`; the symbol gate prevents a
/// double warning (a call the syntactic pass could fully resolve has no symbol arg).
pub(super) fn check_multi_call_inferred(
    heap: &Heap,
    head: value::Symbol,
    items: &[Value],
    info: &MultiInfo,
    ctx: &super::ctx::Ctx,
    pos: Option<Pos>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(mname) = info.generics.get(&head) else {
        return;
    };
    let args = items.get(1..).unwrap_or(&[]);
    if args.is_empty() || !args.iter().any(|a| matches!(a, Value::Sym(_))) {
        return;
    }
    let tuple: Option<Vec<String>> = args
        .iter()
        .map(|&a| multi_arg_identity(heap, a, info, ctx))
        .collect();
    if let Some(tuple) = tuple {
        if info.missing(mname, &tuple) {
            out.push((pos, multi_missing_warning(mname, &tuple)));
        }
    }
}
