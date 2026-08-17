//! Inferred module loading from qualified references (ADR-227 follow-up).
//!
//! A qualified reference `mod/name` **infers a load of `mod`** — you never write a load
//! line to satisfy a `mod/…` reference; naming where something comes from loads it on
//! demand. This holds for every module (`json`, `set`, your own project modules, the
//! curated stdlib). The load goes through the internal loader (`require-one`); there is
//! **no user-facing `require` form** (removed). `(:use mod)` still exists — it
//! additionally refers the module's names *bare* — and loads via the same internal loader.
//!
//! There is no bare-name magic: a bare `sqrt` with neither a `math/` prefix nor
//! `(:use math)` stays unbound.
//!
//! ## Functions vs macros — the compile-time ordering
//!
//! A qualified **function** is only needed at eval time, so its module can be required
//! *after* resolve. A qualified **macro** expands during `macroexpand_all`, so its
//! module must be loaded *before* expansion — the reason a compile-time `require` is
//! elsewhere made mandatory for macros. We infer it instead of demanding it. Three
//! hooks, each firing only on a `/` in a symbol:
//!
//! 1. [`require_qualified_head`] — called from `macroexpand_1` for a qualified call head
//!    into a not-yet-loaded module, so a qualified macro (or any qualified call) loads
//!    *before* the macro lookup. Eager (requires immediately).
//! 2. [`record_qualified`] — called from `resolve_sym` for a qualified reference it
//!    resolves (a value in argument position, a macro-injected reference). Deferred:
//!    records the module on a thread-local buffer, drained by [`drain_pending`] after
//!    resolve, before eval.
//! 3. [`scan_root_refs`] — at the root region (a header-less script / the REPL, where
//!    `resolve` is identity), scans the form for qualified references so a top-level
//!    qualified value auto-requires too. Gated so it never runs during prelude boot.
//!
//! See `docs/auto-derived-imports.md`.

use std::cell::{Cell, RefCell};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Symbol, Value, ValueRef};
use crate::error::LispResult;

thread_local! {
    /// Whether `resolve_sym` should *record* the qualified references it resolves. Off
    /// for read-only callers (the LSP's `resolve_reference`), which must not enqueue a
    /// module load nobody will drain.
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    /// Modules a qualified reference in the current compile pass pulled in, deduped,
    /// drained by [`drain_pending`].
    static PENDING: RefCell<Vec<Symbol>> = const { RefCell::new(Vec::new()) };
}

/// The module a qualified reference `s` should auto-require, if any. `None` for a bare
/// name, a root-escape (`/foo`) or bare `/`, or an alias prefix (`m/…` from
/// `(require … :as m)` — its target was loaded by that require). Otherwise the module
/// symbol, **rooted** for an intra-package reference (ADR-070). The module is everything
/// before the *last* slash, so `net/http/get` yields `net/http`.
pub fn module_to_require(heap: &Heap, s: Symbol) -> Option<Symbol> {
    let name = value::symbol_name_ref(s);
    let first = name.find('/')?;
    if first == 0 {
        return None; // `/foo` root-escape, or the bare `/` operator
    }
    let alias_key = value::intern(&format!("{}/", &name[..first]));
    if heap.import_of(alias_key).is_some() {
        return None; // alias prefix — its target module is already loaded
    }
    // Root an intra-package reference to its package (external/std left as-is).
    let full = heap.root_qualified_ref(s).unwrap_or(s);
    let full_name = value::symbol_name_ref(full);
    let last = full_name.rfind('/')?;
    if last == 0 {
        return None;
    }
    Some(value::intern(&full_name[..last]))
}

/// Eagerly `require` the module of a qualified **call head** `s` if it is not yet loaded
/// — the compile-time hook that makes a qualified *macro* expand without an explicit
/// `require` (and loads a qualified function head early, harmlessly). A no-op when `s`
/// is bare, already bound (module loaded), or an alias/root-escape. Cheap: only an
/// unbound symbol with a `/` triggers a load.
pub fn require_qualified_head(heap: &mut Heap, env: EnvId, s: Symbol) -> LispResult {
    // Already bound ⇒ its module is loaded (or it is a prelude/root name) — nothing to do.
    if heap.env_get(EnvId::GLOBAL, s).is_some() {
        return Ok(Value::nil());
    }
    let rooted = heap.root_qualified_ref(s).unwrap_or(s);
    if rooted != s && heap.env_get(EnvId::GLOBAL, rooted).is_some() {
        return Ok(Value::nil());
    }
    let Some(module) = module_to_require(heap, s) else {
        return Ok(Value::nil());
    };
    // A reference to our own module's (forward-declared) name — it is mid-load, so never
    // re-require it (mirrors `record_qualified`'s self-namespace filter).
    if heap.compile_ns() == Some(module) {
        return Ok(Value::nil());
    }
    ensure_required(heap, env, module)?;
    Ok(Value::nil())
}

/// Record the module of a resolved qualified reference `qualified_name` for auto-require
/// (deferred; drained by [`drain_pending`]). A no-op unless recording is armed. Pure —
/// interns a symbol and pushes to a thread-local, no heap/GC work — so it is safe to
/// call from `resolve_sym` under its GC/macro blocks.
///
/// `current_ns` is the namespace being compiled: a module's reference to **its own**
/// qualified name (`project/foo` inside `project.blsp`) must NOT auto-require itself —
/// the module is mid-load, so re-requiring it is a spurious "still loading" cycle. Skip
/// the record when the reference's module is the current namespace.
pub fn record_qualified(qualified_name: &str, current_ns: &str) {
    if !RECORDING.with(|recording| recording.get()) {
        return;
    }
    let Some(slash) = qualified_name.rfind('/') else {
        return;
    };
    if slash == 0 {
        return;
    }
    let module = &qualified_name[..slash];
    if module == current_ns {
        return; // a reference to our own module — already loading, never re-require
    }
    record_module(value::intern(module));
}

fn record_module(module: Symbol) {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if !pending.contains(&module) {
            pending.push(module);
        }
    });
}

/// Scan a **root-region** form (a header-less script or REPL input, where there is no
/// namespace and `resolve` is identity) for a qualified reference, recording each so it
/// auto-requires — the same convenience a `defmodule` file gets from `resolve_sym`,
/// extended to bare scripts and the REPL. Records only; the form is unchanged.
///
/// Gated: a no-op unless recording is armed **and** the prelude is up (`require-one`
/// bound) — so it never walks or requires during the bulk of prelude boot. Quoted /
/// quasiquoted subtrees are treated as data and skipped, so `'math/foo` does not load
/// `math`.
pub fn scan_root_refs(heap: &Heap, form: Value) {
    if !RECORDING.with(|recording| recording.get()) {
        return;
    }
    if heap
        .env_get(EnvId::GLOBAL, value::intern("require-one"))
        .is_none()
    {
        return; // prelude not up yet — nothing can be required anyway
    }
    scan_refs(heap, form);
}

fn scan_refs(heap: &Heap, form: Value) {
    match form.unpack() {
        ValueRef::Sym(s) => {
            if let Some(module) = module_to_require(heap, s) {
                record_module(module);
            }
        }
        ValueRef::Pair(_) => {
            let Ok(items) = heap.list_to_vec(form) else {
                return;
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
                let hn = value::symbol_name_ref(h);
                if hn == crate::core::keywords::QUOTE || hn == crate::core::keywords::QUASIQUOTE {
                    return; // quoted data is not a reference
                }
            }
            for it in items {
                scan_refs(heap, it);
            }
        }
        ValueRef::Vector(id) => {
            for it in heap.vector(id).to_vec() {
                scan_refs(heap, it);
            }
        }
        ValueRef::Map(id) => {
            for (k, v) in heap.map_entries(id) {
                scan_refs(heap, k);
                scan_refs(heap, v);
            }
        }
        ValueRef::Set(id) => {
            for it in heap.set_elems(id) {
                scan_refs(heap, it);
            }
        }
        _ => {}
    }
}

/// Scope that arms recording of inferred requires for one `resolve`/root-scan pass.
/// Clears any stale pending state on entry (a previous pass that errored between resolve
/// and drain), and restores the prior recording flag on drop, so a nested compile
/// (triggered by a `require` the drain runs) never leaks its state into the caller's pass.
pub struct RecordingScope {
    previous: bool,
}

impl RecordingScope {
    pub fn enter() -> Self {
        PENDING.with(|pending| pending.borrow_mut().clear());
        let previous = RECORDING.with(|recording| recording.replace(true));
        RecordingScope { previous }
    }
}

impl Drop for RecordingScope {
    fn drop(&mut self) {
        RECORDING.with(|recording| recording.set(self.previous));
    }
}

/// `require` each module a qualified reference recorded during the just-finished resolve
/// pass, so `math/sqrt` is bound before the resolved form is evaluated. Idempotent per
/// module (`require` is a no-op for an already-loaded feature). A no-op — one thread-local
/// take — when nothing was recorded, the common case. Runs after `resolve`'s GC/macro
/// blocks have been dropped, so it may load code.
pub fn drain_pending(heap: &mut Heap, env: EnvId) -> LispResult {
    let pending = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if pending.is_empty() {
        return Ok(Value::nil()); // the common case — no root/env work at all
    }
    // Each `ensure_required` loads a module, which collects. `env` may be a LOCAL
    // frame that the collector relocates, so root it and read it back per iteration
    // rather than holding a stale copy across the loads.
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(env);
    for module in pending {
        let env = heap.read_root_env(env_root);
        if let Err(error) = ensure_required(heap, env, module) {
            heap.truncate_env_roots(env_base);
            return Err(error);
        }
    }
    heap.truncate_env_roots(env_base);
    Ok(Value::nil())
}

/// `(require-one 'module)` from Rust: applies the prelude's loader through the active
/// engine. A no-op if the prelude isn't up yet (`require-one` unbound at boot, before
/// any qualified reference can be compiled anyway).
fn ensure_required(heap: &mut Heap, env: EnvId, module: Symbol) -> LispResult {
    let require_one = value::intern("require-one");
    let Some(loader) = heap.env_get(EnvId::GLOBAL, require_one) else {
        return Ok(Value::nil());
    };
    let root = heap.env_root(env);
    match crate::eval::compile::apply_engine(heap, loader, &[Value::symbol(module)], root) {
        Ok(value) => Ok(value),
        // Best-effort: inferring a require must not turn a reference into a compile error.
        // A module that cannot be found falls through to the normal handling — an in-file
        // module the checker knows without loading, or a genuine typo that surfaces as an
        // ordinary `unbound symbol: mod/name`. A real error *inside* a found module still
        // propagates, so a broken module is never silently hidden behind "unbound".
        Err(error) if error.message.contains("cannot find module") => Ok(Value::nil()),
        // A transitive cycle: a module we auto-require refers back (qualified) into one
        // that is still loading. The reference is being satisfied by that in-progress
        // load itself, so inferring a re-require here must not become a hard error —
        // best-effort, same as an absent module. (The common self-reference case is
        // already filtered upstream in `record_qualified`.)
        Err(error) if error.message.contains("still loading") => Ok(Value::nil()),
        Err(error) => Err(error),
    }
}
