//! Runtime contracts — the binding-time mechanism (ADR-381). A `(sig name T)` is a pure
//! declaration everywhere; whether the declaration is *enforced* is a property of the
//! **binding**, decided when the binding comes to exist, not of the form that declared it.
//!
//! The kernel owns three moments at which a name with a declared arrow signature comes to
//! hold a closure, and consults ONE Brood function at each — `%contract-wrap`, the policy:
//!
//! 1. a global `def` of a closure (`eval.rs`'s `def` arm — every `def` routes through it,
//!    both engines, `load`, `eval`, the REPL);
//! 2. a `%register-sig` that lands on a name already bound to a closure (the `sig` written
//!    below its definition), and `sig!`'s explicit `%contract-force!`;
//! 3. an embedded std module's bindings arriving as a batch — from source or materialised
//!    from the stdlib image — and the prelude's own bindings at runtime boot: the
//!    [`sweep`] over every declaration a module (or the root) owns.
//!
//! The hook is given the qualified name, the closure and the type-form, and returns the
//! value to bind: the closure itself when nothing is to be enforced (contracts unarmed and
//! the name not `sig!`-forced, or the name exempt), or a checking shim. The kernel then
//! binds what it returned, keeping what a `def` records beside a binding — the closure's
//! name and docstring, the `defn-` privacy mark, the `(meta …)` facts — so a contracted
//! name introspects like the plain one. `arglist`/`arity-of` read the shim's own
//! parameters: a fixed-arity shim has the same arity, a variadic one reads `n+` (the
//! `&optional`/`&` declarations).
//!
//! **A shim is recognisable by construction**, not by a table: the prelude's shim
//! templates close over the original under the lexical name `%contract-orig`, so the
//! closure carries its own evidence ([`shim_original`]). That is what keeps re-wrapping
//! from stacking — a `sig!` after a `sig`, a changed declaration, a sweep over a module
//! whose defs already fired the hook — and it survives an `%isolate` restore and a
//! compaction, which a side table of handles would not.
//!
//! Why the kernel and not the `sig` macro: the macro used to *become* a rebinding under
//! `BROOD_CONTRACTS=1` — the reinterpretation ADR-152 removed elsewhere — and every one of
//! ADR-153's problems followed from that: the declaration had to sit BELOW its definition
//! (KI-81, KI-113, `sig_placement.rs`), a `provide`-time queue patched the above-defn case
//! for modules only, the prelude could not carry a contract, a module materialised from the
//! image never evaluated its sigs and so was never contracted, and a hot reload of a
//! contracted function silently dropped its contract. A binding-time hook has none of those:
//! order does not matter, the image path and the source path meet at the same sweep, and a
//! reload is a `def`.
//!
//! **Arming.** `BROOD_CONTRACTS=1` arms every declaration in the process; `nest run` and
//! `nest test` arm it by default (the dev-mode default, ADR-381) and `BROOD_CONTRACTS=0`
//! opts out; a released bundle never arms. Read once, here, so the policy side asks
//! `(%contracts-armed?)` and the kernel never reads the environment on a call path.
//!
//! **The boundary (ADR-383).** A contract guards a MODULE's boundary, not every call: a
//! module's calls to its own functions are inside it — the checker holds them to the
//! declaration statically, and the module's author owes the check only to callers
//! outside. So beside the shim, [`contract_bind`] binds the ORIGINAL under a second,
//! private name — `string/char-at`'s under `string/%orig%char-at` — and the compiler
//! resolves a same-module reference to a contracted sibling to that name
//! (`lower::module_boundary_ref`, reading the closure's authoring module,
//! `Closure::module`). The alias exists exactly while the shim does: bound with it,
//! rebound with every `def` of the public name ([`mirror_alias`]), and answered by the
//! public name on a miss (`derive::global_miss`) should the globals ever roll back under
//! a compiled arm — so a stale alias can only check more, never less.
//! `BROOD_NO_CONTRACT_BOUNDARY=1` keeps every call contracted (the A/B and bisect lever).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Symbol, Value, ValueRef};
use crate::error::{LispError, LispResult};
use crate::eval::compile::apply_engine;

use super::numeric::{arg, expect_string, expect_symbol};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%contracts-armed?",
        Arity::exact(0),
        Sig::new(vec![], bool_ty),
        &[],
        "",
        contracts_armed_p,
    );
    primitives.def(
        "%contract-force!",
        Arity::exact(1),
        Sig::new(vec![sym], sym),
        &[],
        "",
        contract_force_prim,
    );
    primitives.def(
        "%contract-forced?",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        &[],
        "",
        contract_forced_p,
    );
    primitives.def(
        "%contracts-sweep!",
        Arity::exact(1),
        Sig::new(vec![any], int),
        &[],
        "",
        contracts_sweep_prim,
    );
}

/// Whether `BROOD_CONTRACTS=1` armed runtime contracts for this process. Cached: the
/// question is asked on every sig'd `def` and on every ability-op result under a declared
/// return, and a `var_os` on either path would be the cost the flag exists to avoid.
pub fn contracts_armed() -> bool {
    static ARMED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARMED
        .get_or_init(|| std::env::var_os("BROOD_CONTRACTS").is_some_and(|v| v == "1" || v == "all"))
}

/// Whether the PRELUDE's own declarations are enforced too — `BROOD_CONTRACTS=all`. Off
/// under plain `1` on purpose: the prelude's sixteen signed names are the language's hottest
/// vocabulary (`nth`, `conj`, `assoc`, `keys`, `into`, …), each a thin wrapper over a native
/// that already raises the precise error, and a shim on `nth` is a 0.4 → 4.5 µs call — a
/// `json/decode` measured 16× slower with them and 1.4× without (2026-09-21). `all` keeps
/// the mechanism live and gated (`contracts_mode.rs`), for a run that wants every
/// declaration in the process enforced.
pub fn prelude_contracts_armed() -> bool {
    static ARMED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARMED.get_or_init(|| std::env::var_os("BROOD_CONTRACTS").is_some_and(|v| v == "all"))
}

fn contracts_armed_p(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(contracts_armed()))
}

/// The name of the policy hook. Defined in `std/prelude/contracts.blsp`, the last prelude
/// file; unbound while the prelude is being built, and the kernel simply does nothing then.
const HOOK: &str = "%contract-wrap";

/// The lexical name a shim template binds its original under — the convention the
/// prelude's `%contract-shim-*` templates and [`shim_original`] share.
const ORIGINAL: &str = "%contract-orig";

/// What to do when the name's current binding is already a shim.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnShim {
    /// Offer the ORIGINAL to the hook again, so a changed declaration re-wraps without
    /// stacking (`%register-sig`).
    Rewrap,
    /// Leave it — the sweep and `%contract-force!`, whose declarations did not change.
    Keep,
}

/// The original closure a contract shim wraps, if `val` is one. A shim is a closure whose
/// captured frame binds [`ORIGINAL`] — evidence the value carries itself.
fn shim_original(heap: &Heap, val: Value) -> Option<Value> {
    let ValueRef::Fn(id) = val.unpack() else {
        return None;
    };
    let env = heap.closure(id).env?;
    if env == EnvId::GLOBAL {
        return None;
    }
    let orig = heap.env_get(env, value::intern(ORIGINAL))?;
    matches!(orig.unpack(), ValueRef::Fn(_)).then_some(orig)
}

/// The arrow type-form declared for `name`, if any — a `(sig *name* int)` declares a
/// VALUE's type and installs nothing.
fn declared_arrow(heap: &Heap, name: Symbol) -> Option<Value> {
    let t = super::modules::sig_type_of(heap, heap.declared_sig_value(name))?;
    let items = heap.list_to_vec(t).ok()?;
    let arrow = value::intern("->");
    items
        .iter()
        .any(|v| matches!(v, Value::Sym(s) if *s == arrow))
        .then_some(t)
}

/// Give a name's binding to the policy hook and bind what comes back; `true` when a shim
/// was installed. The kernel side of moments 1 and 2 above, and the per-name step of 3.
///
/// Does nothing while the prelude is being BUILT (a shim there would capture a frame the
/// freeze forbids; the prelude's contracts are installed per runtime at boot) or when the
/// hook is not yet defined. The `def` and `%register-sig` callers also stand down while an
/// embedded std module is loading — its bindings are swept once at the end, whichever way
/// they arrived, so the stdlib image, written from such a load, never carries a shim; the
/// sweep itself runs inside an enclosing load when modules nest, which is why that guard
/// is theirs and not this function's.
pub(crate) fn contract_apply(
    heap: &mut Heap,
    name: Symbol,
    on_shim: OnShim,
) -> Result<bool, LispError> {
    if heap.global() != EnvId::GLOBAL {
        return Ok(false);
    }
    // Unarmed and not `sig!`-forced, the hook can only decline — that is its own first test
    // (`%contract-wrap`) — so it is not consulted. This is a load-time hot loop, not a
    // nicety: the offer runs at every declared `def`, and a Brood call per definition put
    // `not` past the JIT's tier threshold while `io` materialised, so every short
    // `brood file` run instantiated Cranelift at boot to compile it (KI-182: `startup` +6%).
    if !contracts_armed() && !heap.is_contract_forced(name) {
        return Ok(false);
    }
    let Some(hook) = heap.env_get(EnvId::GLOBAL, value::intern(HOOK)) else {
        return Ok(false);
    };
    let Some(current) = heap.env_get(EnvId::GLOBAL, name) else {
        return Ok(false);
    };
    if !matches!(current.unpack(), ValueRef::Fn(_)) {
        return Ok(false);
    }
    let subject = match shim_original(heap, current) {
        Some(_) if on_shim == OnShim::Keep => return Ok(false),
        Some(orig) => orig,
        None => current,
    };
    let Some(type_form) = declared_arrow(heap, name) else {
        return Ok(false);
    };
    let wrapped = apply_engine(
        heap,
        hook,
        &[Value::symbol(name), subject, type_form],
        EnvId::GLOBAL,
    )?;
    // The hook returns the closure it was given to decline (unarmed and not forced, or
    // exempt): the same handle, so nothing is rebound and no epoch moves.
    let declined = matches!(
        (wrapped.unpack(), subject.unpack()),
        (ValueRef::Fn(a), ValueRef::Fn(b)) if a.0 == b.0
    );
    if declined {
        return Ok(false);
    }
    contract_bind(heap, name, wrapped);
    Ok(true)
}

/// Bind `shim` as the global `name`, carrying over what the current binding records beside
/// itself: the closure's name and docstring (so `doc`/`apropos`/a trace name the function,
/// not an anonymous shim), the `defn-` privacy mark and the `(meta …)` facts — both of
/// which `env_define` clears on every global define, by design (ADR-146/283).
fn contract_bind(heap: &mut Heap, name: Symbol, shim: Value) {
    let previous = heap.env_get(EnvId::GLOBAL, name);
    let shim = match (shim.unpack(), previous.map(|p| p.unpack())) {
        (ValueRef::Fn(shim_id), Some(ValueRef::Fn(orig_id))) => {
            let doc = heap.closure(orig_id).doc.clone();
            let mut c = heap.closure(shim_id).clone();
            c.name = Some(name);
            c.doc = doc;
            // Own arms for the named copy — the same invariant `name_value` keeps: two live
            // LOCAL closures must not share one arms allocation across a collection.
            c.arms = c.arms.iter().cloned().collect();
            Value::func(heap.alloc_closure(c))
        }
        _ => shim,
    };
    let was_private = heap.is_private(name);
    let meta = heap.name_meta(name);
    heap.env_define(EnvId::GLOBAL, name, shim);
    if was_private {
        heap.mark_private(name);
    }
    if let Some(meta) = meta {
        heap.set_name_meta(name, meta);
    }
    // The boundary (ADR-383): the ORIGINAL stays reachable under the private alias for
    // the module's own code — unless `sig!` forced the name, which has no boundary: the
    // alias then IS the shim, so a same-module site compiled against it checks too.
    // Reserved with its public name, so a same-module call site keeps the un-staged
    // head a reserved callee earns (`lower.rs`, KI-19).
    if let (Some(alias), Some(original)) = (uncontracted_alias(name), previous) {
        let original = if heap.is_contract_forced(name) {
            shim
        } else {
            shim_original(heap, original).unwrap_or(original)
        };
        heap.env_define(EnvId::GLOBAL, alias, original);
        heap.mark_private(alias);
        if heap.is_reserved_global(name) {
            heap.reserve_global(alias);
        }
        BOUNDARY_BOUND.store(true, Ordering::Release);
    }
}

// ===== the module boundary (ADR-383) =========================================================

/// The leaf marker of an uncontracted alias: `string/char-at`'s is `string/%orig%char-at`.
/// Unreadable as a call by construction — no source spells a `%orig%` leaf — and a `%`
/// name, so every listing that hides internals hides it.
const ALIAS_MARK: &str = "%orig%";

/// Set once the first alias is bound in this process. Read by the compiler on every free
/// symbol, so it must be one static load when no contract exists (the unarmed default).
static BOUNDARY_BOUND: AtomicBool = AtomicBool::new(false);

/// Whether same-module references may compile to an uncontracted alias: one is bound, and
/// `BROOD_NO_CONTRACT_BOUNDARY` did not pin every call to the shim.
pub(crate) fn boundary_active() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    BOUNDARY_BOUND.load(Ordering::Acquire)
        && !*DISABLED.get_or_init(|| std::env::var_os("BROOD_NO_CONTRACT_BOUNDARY").is_some())
}

/// The module of a qualified name (`string/char-at` → `string`), `None` for a root name.
pub(crate) fn module_of_qualified(name: Symbol) -> Option<Symbol> {
    crate::eval::derive::module_of_resolved(name)
}

/// The uncontracted alias of a QUALIFIED name; a root name has no module and so no
/// boundary — a script's, or the prelude's, declarations are enforced at every call.
fn uncontracted_alias(name: Symbol) -> Option<Symbol> {
    let text = value::symbol_name_ref(name);
    let split = text.rfind('/')?;
    if split == 0 {
        return None;
    }
    Some(value::intern(&format!(
        "{}/{ALIAS_MARK}{}",
        &text[..split],
        &text[split + 1..]
    )))
}

/// The public name an uncontracted alias stands for, if `sym` is one.
pub(crate) fn alias_public(sym: Symbol) -> Option<Symbol> {
    let text = value::symbol_name_ref(sym);
    let split = text.rfind('/')?;
    let leaf = text[split + 1..].strip_prefix(ALIAS_MARK)?;
    Some(value::intern(&format!("{}/{leaf}", &text[..split])))
}

/// The binding a reference to `sym` from code of `module` compiles to when `sym` is a
/// CONTRACTED function of that same module: its uncontracted alias, which is bound exactly
/// while the shim is. `None` for every other reference — another module's name, a root
/// name, a sibling with no contract.
pub(crate) fn uncontracted_sibling(heap: &Heap, module: Symbol, sym: Symbol) -> Option<Symbol> {
    if module_of_qualified(sym)? != module {
        return None;
    }
    let alias = uncontracted_alias(sym)?;
    heap.env_get(EnvId::GLOBAL, alias)
        .is_some()
        .then_some(alias)
}

/// The module a closure's code belongs to, for the boundary: what it recorded, else the
/// module of its own qualified name (a top-level `def` the tree-walker built). The name
/// half is memoised per symbol — it is a string split, and this runs on a call path.
fn closure_module(heap: &Heap, id: value::ClosureId) -> Option<Symbol> {
    let cl = heap.closure(id);
    cl.module
        .or_else(|| cl.name.and_then(module_of_qualified_cached))
}

/// The thin-wrapper redirect's half of the boundary (ADR-383). A pass-through arm —
/// `(defn g (x) (f x))`, or the callback `(fn (x) (f x))` — never runs its compiled body:
/// the dispatcher forwards the call to the inner head resolved by NAME, so the compile-time
/// alias never applies to it. When that resolution lands on a contract shim of the
/// wrapper's OWN module, this is the binding the wrapper's code meant: the original
/// (`Some`), unless the name was `sig!`-forced. `None` leaves the redirect alone. Costs a
/// static load when no boundary is bound; otherwise the shim test on the resolved value,
/// which is a frame probe, and a module comparison only when it IS a shim.
pub(crate) fn boundary_redirect(
    heap: &Heap,
    wrapper: value::ClosureId,
    inner: Value,
) -> Option<Value> {
    if !boundary_active() {
        return None;
    }
    let original = shim_original(heap, inner)?;
    let ValueRef::Fn(shim_id) = inner.unpack() else {
        return None;
    };
    let name = heap.closure(shim_id).name?;
    if heap.is_contract_forced(name) {
        return None;
    }
    let module = closure_module(heap, wrapper)?;
    (module_of_qualified_cached(name) == Some(module)).then_some(original)
}

/// The lexical name a tree-walked frame binds its closure's module under — the
/// tree-walker's counterpart of the compiled arm's `Scope::module`.
const FRAME_MODULE: &str = "%contract-module";

fn frame_module_symbol() -> Symbol {
    static SYMBOL: std::sync::OnceLock<Symbol> = std::sync::OnceLock::new();
    *SYMBOL.get_or_init(|| value::intern(FRAME_MODULE))
}

/// Bind the closure's module into the frame the tree-walker is about to run its body
/// in, so [`boundary_resolve`] and a nested `(fn …)` can read it. Nothing when no
/// boundary is bound (one static load) or the closure has no module.
pub(crate) fn bind_frame_module(heap: &mut Heap, frame: EnvId, closure: value::ClosureId) {
    if !boundary_active() {
        return;
    }
    if let Some(module) = closure_module(heap, closure) {
        heap.env_define(frame, frame_module_symbol(), Value::symbol(module));
    }
}

/// The module the tree-walked frame chain `env` runs under, if a frame recorded one.
pub(crate) fn frame_module(heap: &Heap, env: EnvId) -> Option<Symbol> {
    if !boundary_active() {
        return None;
    }
    match heap.env_get(env, frame_module_symbol())?.unpack() {
        ValueRef::Sym(module) => Some(module),
        _ => None,
    }
}

/// The tree-walker's half of the boundary: `resolved` is what the name a tree-walked
/// body evaluated resolves to. When that is a contract shim of the module the frame
/// chain `env` runs under — and the name was not `sig!`-forced — the original is the
/// binding the body meant, exactly as a compiled arm's alias reference reads it.
/// Anything else passes through. A static load when no boundary is bound; otherwise a
/// shim test on the value, and the frame walk only when it is one.
pub(crate) fn boundary_resolve(heap: &Heap, env: EnvId, resolved: Value) -> Value {
    if !boundary_active() {
        return resolved;
    }
    let Some(original) = shim_original(heap, resolved) else {
        return resolved;
    };
    let ValueRef::Fn(shim_id) = resolved.unpack() else {
        return resolved;
    };
    let Some(name) = heap.closure(shim_id).name else {
        return resolved;
    };
    if heap.is_contract_forced(name) {
        return resolved;
    }
    match (frame_module(heap, env), module_of_qualified_cached(name)) {
        (Some(here), Some(owner)) if here == owner => original,
        _ => resolved,
    }
}

/// [`module_of_qualified`], memoised per symbol for the call path above.
fn module_of_qualified_cached(name: Symbol) -> Option<Symbol> {
    thread_local! {
        static MEMO: std::cell::RefCell<std::collections::HashMap<Symbol, Option<Symbol>>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    MEMO.with(|memo| {
        *memo
            .borrow_mut()
            .entry(name)
            .or_insert_with(|| module_of_qualified(name))
    })
}

/// Keep the alias in step with a `def` of its public name: whenever `name` is rebound to
/// `val` — a reload, a redefinition to a non-closure — the alias, if bound, follows, so a
/// same-module site compiled against it never calls a superseded body. A closure `def`
/// under a declared arrow then re-wraps through [`contract_apply`], which rebinds both
/// again; this is the step that also covers the rebinding the hook declines (a
/// non-closure) and the one it defers (inside an embedded module's load, until the
/// sweep). One static load when no boundary was ever bound.
pub(crate) fn mirror_alias(heap: &mut Heap, root: EnvId, name: Symbol, val: Value) {
    if !BOUNDARY_BOUND.load(Ordering::Acquire) {
        return;
    }
    let Some(alias) = uncontracted_alias(name) else {
        return;
    };
    if heap.env_get(EnvId::GLOBAL, alias).is_some() {
        heap.env_define(root, alias, val);
    }
}

/// `(%contract-force! 'name)` — `sig!`'s runtime half: enforce `name`'s declared signature
/// whatever the mode and at every call. Marks the name forced (so a later or re-`def`inition
/// is wrapped when it lands), offers its binding to the policy now, and points the
/// uncontracted alias at the shim: `%register-sig` may already have installed the shim
/// under the armed default, with the alias on the original, and a forced name has no
/// boundary — its own module calls the shim too. Returns the qualified name.
fn contract_force_prim(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%contract-force!", arg(args, 0))?;
    let name = crate::eval::macros::resolve_reference(heap, name);
    heap.force_contract(name);
    contract_apply(heap, name, OnShim::Keep)?;
    if let (Some(alias), Some(current)) =
        (uncontracted_alias(name), heap.env_get(EnvId::GLOBAL, name))
    {
        if shim_original(heap, current).is_some() && heap.env_get(EnvId::GLOBAL, alias).is_some() {
            heap.env_define(EnvId::GLOBAL, alias, current);
        }
    }
    Ok(Value::symbol(name))
}

/// `(%contract-forced? 'name)` — was `name`'s contract forced by a `sig!`?
fn contract_forced_p(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%contract-forced?", arg(args, 0))?;
    Ok(Value::boolean(heap.is_contract_forced(name)))
}

/// The declared signatures owned by `owner` — the module whose qualified names are
/// `owner/…` — or, for `owner` None, the ROOT names (no slash), which is what the prelude
/// declares. Reads both stores, so a prelude declaration and a runtime one answer alike.
fn declared_names_of(heap: &Heap, owner: Option<&str>) -> Vec<Symbol> {
    let prefix = owner.map(|m| format!("{m}/"));
    heap.declared_sigs_everywhere()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| {
            let text = value::symbol_name(*name);
            match &prefix {
                Some(p) => text
                    .strip_prefix(p.as_str())
                    .is_some_and(|rest| !rest.contains('/')),
                None => !text.contains('/'),
            }
        })
        .collect()
}

/// Moment 3: offer every declared name `owner` owns to the policy, leaving a name that is
/// already a shim alone. Returns how many shims were installed. Called from Brood after an
/// embedded module's bindings exist (`require-one`, both the source and the image branch)
/// and from [`crate::Interp::new`] for the prelude's root names when contracts are armed.
pub(crate) fn sweep(heap: &mut Heap, owner: Option<&str>) -> Result<usize, LispError> {
    let mut installed = 0;
    for name in declared_names_of(heap, owner) {
        if contract_apply(heap, name, OnShim::Keep)? {
            installed += 1;
        }
    }
    Ok(installed)
}

/// `(%contracts-sweep! owner)` — [`sweep`] from Brood; `owner` a module name string, or nil
/// for the root names.
fn contracts_sweep_prim(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let owner = match arg(args, 0) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%contracts-sweep!", v)?),
    };
    let n = sweep(heap, owner.as_deref())?;
    Ok(Value::int(n as i64))
}
