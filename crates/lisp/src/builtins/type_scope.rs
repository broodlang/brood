//! Where a bare type name in a `sig` or a `deftype` body is resolved.
//!
//! A `(sig parse-body (conn -> conn))` in module `web/conn` means `web/conn`'s `conn`. The
//! runtime contract, though, checks the value wherever the call happens, holding only the
//! type-form — and a bare `conn` there used to be looked up as an alias across every loaded
//! module first, so another module's `(deftype conn …)` answered for this module's record
//! and rejected it. So the form is SCOPED where it is declared, before any contract reads
//! it: each bare name that means something in the declaring module is replaced by its
//! qualified name, nearest first —
//!
//! 1. the module's own `deftype` or `defrecord` (`web/conn/conn`);
//! 2. the ONE `(:use …)`d module that declares it (two decline, leaving the name bare);
//! 3. otherwise the name stays bare, and keeps its old meaning: a base type (which no module
//!    can rebind, so it is never scoped), an ability (registered by the name its author
//!    wrote), or the one alias anywhere by that name.
//!
//! A qualified name reaches exactly one alias or record id, so the contract's lookups and
//! its verdict memo (keyed by the type-form's identity) need no scope of their own.
//!
//! **Timing.** A declaration is scoped when it is registered (`%register-sig`,
//! `%register-type`). A name its module has not declared YET — a `sig` above the
//! `defrecord` it names — is remembered (`Heap::note_type_scope_pending`) and the
//! declaration is scoped again when `ns/name` arrives (`%register-type`, `%record-register`).
//! While the module is still loading under `require`, a used module's name is not taken
//! yet either, since the module itself may still declare it; `provide` settles those
//! (`%scope-types!`). A declaration whose stored form changes is offered to the contract
//! hook again, so a shim built from the old form is rebuilt.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Symbol, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_symbol};

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%type-name-arrived!",
        Arity::exact(1),
        Sig::new(vec![any], nil_ty),
        &[],
        "",
        type_name_arrived_prim,
    );
    primitives.def(
        "%scope-types!",
        Arity::exact(1),
        Sig::new(vec![any], nil_ty),
        &[],
        "",
        scope_types_prim,
    );
}

/// The module a declaration key belongs to (`web/conn` for `web/conn/parse-body`), or
/// `None` for a root name, which has no module to scope in.
fn namespace_of(key: Symbol) -> Option<Symbol> {
    value::symbol_name_ref(key)
        .rsplit_once('/')
        .filter(|(ns, _)| !ns.is_empty())
        .map(|(ns, _)| value::intern(ns))
}

/// Does `qualified` name a type — an alias (`deftype`) or a record id (`defrecord`)?
fn names_a_type(heap: &Heap, qualified: Symbol) -> bool {
    if super::tooling::alias_form_at(heap, qualified).is_some() {
        return true;
    }
    match heap.env_get(
        EnvId::GLOBAL,
        value::intern(crate::core::registries::RECORD_IDS),
    ) {
        Some(Value::Map(map_id)) => heap.map_get(map_id, Value::Keyword(qualified)).is_some(),
        _ => false,
    }
}

/// What the scope walk needs to know about the declaring module.
struct Scope {
    ns: Symbol,
    uses: Vec<Symbol>,
    /// Whether a used module's name may be taken now (false while the module is loading).
    take_uses: bool,
    /// The declaration being scoped, noted against a name that is not resolved yet.
    key: Symbol,
    /// Undo the scoping instead: a qualified name its bare spelling would resolve to goes
    /// back to that spelling (`unscope_form`). Notes nothing.
    unscope: bool,
}

/// The qualified name a bare `name` means in `scope`, or `None` to leave it as written.
fn resolve(heap: &Heap, scope: &Scope, name: Symbol) -> Option<Symbol> {
    let spelled = value::symbol_name_ref(name);
    if spelled.contains('/')
        || spelled.starts_with('?')
        || spelled.starts_with('&')
        || spelled == "->"
        || crate::core::heap::is_base_type_name(name)
    {
        return None;
    }
    let own = value::intern(&format!("{}/{spelled}", value::symbol_name_ref(scope.ns)));
    if names_a_type(heap, own) {
        return Some(own);
    }
    // Not the module's own — yet. A later `deftype`/`defrecord` of it scopes this again.
    if !scope.unscope {
        heap.note_type_scope_pending(scope.ns, name, scope.key);
    }
    if !scope.take_uses {
        return None;
    }
    let mut hits = scope
        .uses
        .iter()
        .map(|used| value::intern(&format!("{}/{spelled}", value::symbol_name_ref(*used))))
        .filter(|qualified| names_a_type(heap, *qualified));
    let first = hits.next()?;
    if hits.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// `form` with its bare type names scoped, or `None` when nothing changed (so an unchanged
/// form keeps its identity). `bound` holds the `(rec X …)` binders in force.
fn rewrite(
    heap: &mut Heap,
    scope: &Scope,
    form: Value,
    bound: &mut Vec<Symbol>,
    depth: usize,
) -> Option<Value> {
    if depth > 64 {
        return None;
    }
    match form {
        Value::Sym(name) if scope.unscope => {
            let (_, bare) = value::symbol_name_ref(name).rsplit_once('/')?;
            let bare = value::intern(bare);
            (resolve(heap, scope, bare) == Some(name)).then(|| Value::symbol(bare))
        }
        Value::Sym(name) if !bound.contains(&name) => resolve(heap, scope, name).map(Value::symbol),
        Value::Pair(_) => {
            let items = heap.list_to_vec(form).ok()?;
            let is = |item: Option<&Value>, spelled: &str| matches!(item, Some(Value::Sym(s)) if value::symbol_name_ref(*s) == spelled);
            // An arrow's every position is a type; any other list's head is the type
            // constructor (`or`, `record`, `list`, `rec`), and a `(rec X …)` binds `X`.
            let arrow = items.iter().any(|item| is(Some(item), "->"));
            let binder = match items.as_slice() {
                [Value::Sym(head), Value::Sym(x), ..]
                    if !arrow && value::symbol_name_ref(*head) == "rec" =>
                {
                    Some(*x)
                }
                _ => None,
            };
            let skip = if arrow {
                0
            } else if binder.is_some() {
                2
            } else {
                1
            };
            if let Some(x) = binder {
                bound.push(x);
            }
            let mut out = items.clone();
            let mut changed = false;
            for (slot, &item) in out.iter_mut().zip(items.iter()).skip(skip) {
                if let Some(scoped) = rewrite(heap, scope, item, bound, depth + 1) {
                    *slot = scoped;
                    changed = true;
                }
            }
            if binder.is_some() {
                bound.pop();
            }
            changed.then(|| heap.list(out))
        }
        _ => None,
    }
}

/// `form`, the type-form declaration `key` carries, with its bare type names scoped in
/// `key`'s module — or `None` when nothing in it changes (a root name scopes nothing).
/// `settled`: the module has finished loading, so a used module's name may be taken even
/// if the load is still marked in flight.
pub(crate) fn scope_form(
    heap: &mut Heap,
    key: Symbol,
    form: Value,
    settled: bool,
) -> Option<Value> {
    let ns = namespace_of(key)?;
    let scope = Scope {
        ns,
        uses: heap.module_uses(ns),
        take_uses: settled || !super::modules::module_is_loading(heap, value::symbol_name_ref(ns)),
        key,
        unscope: false,
    };
    rewrite(heap, &scope, form, &mut Vec::new(), 0)
}

/// Scope the stored declaration `key` again, store it when it changed, and offer a changed
/// signature's binding to the contract hook, so its shim is rebuilt from the scoped form.
fn rescope(heap: &mut Heap, key: Symbol, settled: bool) -> Result<(), LispError> {
    let Some(entry) = heap.declared_sig_value(key) else {
        return Ok(());
    };
    if let Some(inner) = super::tooling::alias_form_at(heap, key) {
        if let Some(scoped) = scope_form(heap, key, inner, settled) {
            let marker = Value::symbol(value::intern(super::modules::TYPE_ALIAS_MARKER));
            let wrapped = heap.list(vec![marker, scoped]);
            heap.set_declared_sig(key, wrapped);
        }
        return Ok(());
    }
    let Some(type_form) = super::modules::sig_type_of(heap, Some(entry)) else {
        return Ok(());
    };
    let Some(scoped) = scope_form(heap, key, type_form, settled) else {
        return Ok(());
    };
    let props = super::modules::sig_props_of(heap, Some(entry));
    let stored = if props.is_empty() {
        scoped
    } else {
        super::modules::wrap_sig(heap, scoped, &props)
    };
    heap.set_declared_sig(key, stored);
    if !heap.in_module_load() {
        super::contracts::contract_apply(heap, key, super::contracts::OnShim::Rewrap)?;
    }
    Ok(())
}

/// The type `qualified` (`ns/name`) has just been declared: scope again every declaration
/// of `ns` that was waiting on the bare `name`.
pub(crate) fn type_name_arrived(heap: &mut Heap, qualified: Symbol) -> Result<(), LispError> {
    let Some((ns, name)) = value::symbol_name_ref(qualified).rsplit_once('/') else {
        return Ok(());
    };
    let (ns, name) = (value::intern(ns), value::intern(name));
    for key in heap.take_type_scope_pending(ns, Some(name)) {
        rescope(heap, key, false)?;
    }
    Ok(())
}

/// `(%type-name-arrived! id)` — a `defrecord`'s id (`:ns/name`) or a qualified symbol.
fn type_name_arrived_prim(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let qualified = match arg(args, 0) {
        Value::Keyword(k) => k,
        other => expect_symbol(heap, "%type-name-arrived!", other)?,
    };
    type_name_arrived(heap, qualified)?;
    Ok(Value::nil())
}

/// `(%scope-types! module)` — the module has finished loading: scope again every
/// declaration of it still waiting on a name, now that a used module's may be taken.
fn scope_types_prim(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let ns = match arg(args, 0) {
        Value::Str(id) => value::intern(heap.string(id)),
        other => expect_symbol(heap, "%scope-types!", other)?,
    };
    for key in heap.take_type_scope_pending(ns, None) {
        rescope(heap, key, true)?;
    }
    Ok(Value::nil())
}

/// `form`, a stored declaration of `key`, back as its author wrote it: every name the
/// scoping qualified is bare again, since its bare spelling resolves to it in `key`'s
/// module. What an editor shows (`%declared-sig`, `%type-aliases`); a name the author
/// qualified that the bare spelling would not reach stays qualified.
pub(crate) fn unscope_form(heap: &mut Heap, key: Symbol, form: Value) -> Value {
    let Some(ns) = namespace_of(key) else {
        return form;
    };
    let scope = Scope {
        ns,
        uses: heap.module_uses(ns),
        take_uses: true,
        key,
        unscope: true,
    };
    rewrite(heap, &scope, form, &mut Vec::new(), 0).unwrap_or(form)
}
