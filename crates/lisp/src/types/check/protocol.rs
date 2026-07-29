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
        // Every declared op must be implemented, at a compatible arity.
        for op in &proto.ops {
            match provided.get(&op.name) {
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
    })
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
// (not eval'd at check time) with the runtime `ability/*impls*` registry (cross-file
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
                Some("impl-for") => Some(1),
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
            if sym_name(Value::Sym(h)).as_deref() == Some("register-impl") {
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

/// Union in the runtime `ability/*impls*` registry — `[A op] → {id → …}` — so an impl
/// reachable through a required module (not present as a form in this file) counts.
fn read_impls_registry(
    heap: &Heap,
    impls: &mut HashSet<(String, String, String)>,
    defaults: &mut HashSet<(String, String)>,
) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("ability/*impls*"))
    else {
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

/// Sealed abilities' member ids, keyed by ability name → the (qualified) member id name
/// strings. Unions the file's own `register-sealed` forms (in the expanded tree, where the
/// ids are already ns-qualified) with the runtime `ability/*sealed*` registry (imported
/// abilities). Feeds `annot`'s ability-name-as-a-type resolution (a sealed ability is a
/// finite union of its members' record shapes).
pub(super) fn sealed_member_ids(heap: &Heap, expanded: &[Value]) -> HashMap<String, Vec<String>> {
    let mut sealed = HashMap::new();
    for &form in expanded {
        collect_register_sealed(heap, form, &mut sealed);
    }
    read_sealed_registry(heap, &mut sealed);
    sealed
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
    /// ability name → its SEALED member id names (closed set), if declared sealed.
    sealed: HashMap<String, Vec<String>>,
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
    // `(ability, op)` → its `:-> RET` return-type *form* (unparsed). Filled from this
    // file's `register-ability` forms and the runtime registry, then parsed to `Ty`.
    let mut ret_forms: HashMap<(String, String), Value> = HashMap::new();
    for &form in expanded {
        collect_register_ability(heap, form, &mut abilities, &mut ret_forms);
        collect_register_sealed(heap, form, &mut sealed);
    }
    read_abilities_registry(heap, &mut abilities, &mut ret_forms);
    read_sealed_registry(heap, &mut sealed);
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
        sealed,
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
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(h)) = items.first() {
            if sym_name(Value::Sym(h)).as_deref() == Some("register-ability") {
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
                        ops.push(op);
                    }
                    out.insert(a, ops);
                }
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            collect_register_ability(heap, item, out, rets);
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
            if sym_name(Value::Sym(h)).as_deref() == Some("register-sealed") {
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

/// Union in the runtime `ability/*abilities*` registry — name → op specs — recording each
/// op's name and its `:-> RET` return-type form (the latter into `rets`).
fn read_abilities_registry(
    heap: &Heap,
    out: &mut HashMap<String, Vec<String>>,
    rets: &mut HashMap<(String, String), Value>,
) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("ability/*abilities*"))
    else {
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
                ops.push(op);
            }
            out.entry(a).or_insert(ops);
        }
    }
}

/// Union in the runtime `ability/*sealed*` registry — name → member id keywords.
fn read_sealed_registry(heap: &Heap, out: &mut HashMap<String, Vec<String>>) {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern("ability/*sealed*"))
    else {
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
/// declared op (a `:default` doesn't count — sealing means each member is handled
/// explicitly). One warning per missing (ability, op, member).
pub(super) fn check_sealed(info: &AbilityInfo, out: &mut Vec<(Option<Pos>, String)>) {
    for (ability, members) in &info.sealed {
        let Some(ops) = info.abilities.get(ability) else {
            continue;
        };
        for op in ops {
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
