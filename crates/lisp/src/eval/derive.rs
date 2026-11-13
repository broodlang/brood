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
//! ## When the load happens — lazy by default (ADR-335)
//!
//! A qualified reference is a promise that `mod/name` is bound WHEN IT IS EVALUATED, and the
//! load is inferred at the latest point that keeps that promise. Under the default LAZY
//! policy hook 2 only records, and hook 1 defers a head that an opened image vouches is a
//! function ([`image_says_function`]); the load then happens on the global-lookup miss —
//! [`global_miss`], which every engine's unbound arm calls — or, for an arm about to go
//! native, at its tiering election (`preload_arm_globals`). A macro head, or a head into a
//! module no image describes, still loads at expansion. Under the EAGER policy
//! ([`EagerLoadScope`], `%eager-loads!`, `BROOD_NO_LAZY_LOAD=1`) the hooks load at once;
//! the checker runs eager so its unbound verdict sees the modules a file names.
//! Either way an inferred load records no require-edge ([`take_skip_next_edge`]): the image
//! replays edges so a materialised module has what its source load pulled in, and a body
//! reference pulls nothing in.
//!
//! See `docs/auto-derived-imports.md`.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Symbol, Value, ValueRef};
use crate::error::{LispError, LispResult};

thread_local! {
    /// Whether `resolve_sym` should *record* the qualified references it resolves. Off
    /// for read-only callers (the LSP's `resolve_reference`), which must not enqueue a
    /// module load nobody will drain.
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    /// Modules a qualified reference in the current compile pass pulled in, deduped,
    /// drained by [`drain_pending`].
    static PENDING: RefCell<Vec<(Symbol, Symbol)>> = const { RefCell::new(Vec::new()) };
    /// Bare names the current compile pass used that are `(:use …)`-imported from two or
    /// more modules at once (ADR-235): `(bare, sorted candidate qualifieds)`. Recorded by
    /// `resolve_sym` at the point the ambiguous name is referenced, raised as a use-site
    /// error by [`take_resolve_error`]. Only armed under `RECORDING` (the read-only LSP
    /// path must not hard-error a hover).
    static PENDING_AMBIGUOUS: RefCell<Vec<(Symbol, Vec<Symbol>)>> =
        const { RefCell::new(Vec::new()) };
    /// A bare definition head in a module that spells a dynamic variable visible there from
    /// elsewhere (ADR-722): `(bare, the variable it names, module)`. Recorded by the resolve
    /// pass, raised by [`take_resolve_error`].
    static PENDING_DYNAMIC_DEF: Cell<Option<(Symbol, Symbol, Symbol)>> = const { Cell::new(None) };
    /// Set by [`ensure_required`] for the load it is about to run, consumed by the loader's
    /// next `%require-record-edge!` (via [`take_skip_next_edge`]): an inferred load is not a
    /// load-time edge. One-shot, so the edges of the loaded module's OWN header clauses are
    /// recorded as usual.
    static SKIP_NEXT_EDGE: Cell<bool> = const { Cell::new(false) };
    /// Set by [`parkable_miss`] for the load its miss is about to run, consumed beside
    /// `SKIP_NEXT_EDGE` by that `require-one` (via [`take_load_hand_off`]): if it then LOSES
    /// the claim to another process, it answers `:%hand-off` instead of waiting, since its
    /// wait would be a `receive` under this native lookup. One-shot, so no load nested
    /// inside a won claim can hand off.
    static HAND_OFF_ARMED: Cell<bool> = const { Cell::new(false) };
    /// Set by [`ensure_required`] when the load it ran answered `:%hand-off`; read and
    /// cleared by [`parkable_miss`].
    static HANDED_OFF: Cell<bool> = const { Cell::new(false) };
}

/// Consume the one-shot "this load is inferred, record no edge" flag. Called by the
/// `%load-edge-skipped?` primitive at the top of `require-one`.
pub fn take_skip_next_edge() -> bool {
    SKIP_NEXT_EDGE.with(|skip| skip.replace(false))
}

/// Consume the one-shot "a lost claim may hand off" flag. Called by the
/// `%load-may-hand-off?` primitive at the top of `require-one`.
pub fn take_load_hand_off() -> bool {
    HAND_OFF_ARMED.with(|armed| armed.replace(false))
}

// Modules a require has already failed to FIND, with nothing loaded since — see
// [`ensure_required`], which is the only reader. Shared by the runtime's processes
// (`Heap::absent_modules`), never by two runtimes: whether a module can be found depends on
// `*load-path*`, a global of one runtime (KI-226).

fn module_known_absent(heap: &Heap, module: Symbol) -> bool {
    crate::core::sync::read(heap.absent_modules()).contains(&module)
}

fn note_module_absent(heap: &Heap, module: Symbol) {
    crate::core::sync::write(heap.absent_modules()).insert(module);
}

/// Forget every recorded absence. Called when the inputs to module resolution change: a
/// successful load (in [`ensure_required`]) and a load-path change (`reflect/set-load-path`).
/// Clearing wholesale rather than per-module is deliberate — a new load path can make any
/// number of previously-absent modules resolvable, and the set is small.
pub fn clear_absent_modules(heap: &Heap) {
    crate::core::sync::write(heap.absent_modules()).clear();
}

/// The module a qualified reference `s` should auto-require, if any. `None` for a bare
/// name, a root-escape (`/foo`) or bare `/`, or an alias prefix (`m/…` from
/// `(require … :as m)` — its target was loaded by that require). Otherwise the module
/// symbol, **rooted** for an intra-package reference (ADR-070). The module is everything
/// before the *last* slash, so `http/get` yields `http`.
pub fn module_to_require(heap: &Heap, s: Symbol) -> Option<Symbol> {
    let name = value::symbol_name_ref(s);
    let first = name.find('/')?;
    if first == 0 {
        return None; // `/foo` root-escape, or the bare `/` operator
    }
    // A file with no alias cannot hit, so the `prefix/` key is spelled only when it might.
    if heap.imports_may_have_aliases()
        && heap
            .import_of(value::intern(&format!("{}/", &name[..first])))
            .is_some()
    {
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
///
/// Answers whether it required a module: `false` means it bound nothing, so a macro-head
/// lookup the caller made before it still stands.
pub fn require_qualified_head(heap: &mut Heap, env: EnvId, s: Symbol) -> Result<bool, LispError> {
    // Already bound ⇒ its module is loaded (or it is a prelude/root name) — nothing to do.
    if heap.env_get(EnvId::GLOBAL, s).is_some() {
        return Ok(false);
    }
    let rooted = heap.root_qualified_ref(s).unwrap_or(s);
    if rooted != s && heap.env_get(EnvId::GLOBAL, rooted).is_some() {
        return Ok(false);
    }
    let Some(module) = module_to_require(heap, s) else {
        return Ok(false);
    };
    // A reference to our own module's (forward-declared) name — it is mid-load, so never
    // re-require it (mirrors `record_qualified`'s self-namespace filter).
    if heap.compile_ns() == Some(module) {
        return Ok(false);
    }
    // Lazy policy (ADR-335): a head an image vouches is a FUNCTION waits for its first call
    // — the miss path (`global_miss`) loads it then. Only a macro, or a head into a module
    // no index describes, must load before expansion. With no image open, an embedded std
    // module's own source is the index: what it defines with a top-level `defmacro`.
    if lazy_loads() {
        index_embedded_kinds(heap, module);
        if heap.image_says_function(module, rooted) {
            return Ok(false);
        }
    }
    ensure_required(heap, env, module)?;
    Ok(true)
}

/// Give the kind index an EMBEDDED std module no image has indexed, read from the source the
/// binary carries: the module, and the names its top-level `defmacro`/`defmacro-` forms
/// define. A std module always loads from that embedded source, so this is what an image
/// built from it would say. Without it a source boot loaded, at expansion, every module a
/// prelude macro's expansion names (`file`, `path`, `regex`, `datetime`, `tz`, `os`, …) where
/// an imaged boot deferred them to first use. The two boots then disagreed about what was
/// loaded, and every test comparing them carried a hand-kept list of modules to load first.
///
/// The scan sees only top-level `defmacro` forms. `std_macro_kinds_match_the_loaded_modules`
/// (crates/lisp/tests) loads every std module and fails if a macro arrives any other way.
fn index_embedded_kinds(heap: &Heap, module: Symbol) {
    if heap.image_indexes(module) {
        return;
    }
    let name = value::symbol_name_ref(module);
    let Some(src) = crate::builtins::modules::embedded_module_source(name) else {
        return;
    };
    let macros = embedded_macro_names(name, src);
    heap.register_image_kinds([module], macros);
}

/// The qualified names `src` (the source of module `module`) defines with a top-level
/// `defmacro` or `defmacro-`. A name already qualified is kept as written.
pub fn embedded_macro_names(module: &str, src: &str) -> Vec<Symbol> {
    let root = crate::syntax::cst::parse(src);
    root.forms()
        .filter(|form| matches!(form.head_symbol(src), Some("defmacro" | "defmacro-")))
        .filter_map(|form| {
            let name = form.forms().nth(1)?;
            if name.kind != crate::syntax::cst::NodeKind::Symbol {
                return None;
            }
            let name = name.text(src);
            Some(if name.contains('/') {
                value::intern(name)
            } else {
                value::intern(&format!("{module}/{name}"))
            })
        })
        .collect()
}

/// Load the embedded std module a `def` TARGET names, if it is not loaded yet, so the
/// reserved-name check (ADR-166) sees that module's surface. A name is sealed when its module
/// loads; before this, `(def string/join …)` in a process that had not loaded `string` was
/// accepted, and the module's own definition then overwrote it, or it shadowed the module's
/// function until then — whichever ran first decided. A qualified *reference* already loads
/// its module (the head hook above, the miss path); this is the same inference for a `def`.
///
/// Only a prefix that names an embedded module loads: a `def` under your own namespace
/// (`myns/foo`) is a new global, not a reference to a module that must exist. A bound target
/// needs nothing — its module is loaded, or it is yours.
pub fn require_def_target(heap: &mut Heap, env: EnvId, target: Symbol) -> Result<(), LispError> {
    if heap.env_get(EnvId::GLOBAL, target).is_some() {
        return Ok(());
    }
    let Some(module) = module_to_require(heap, target) else {
        return Ok(());
    };
    if heap.compile_ns() == Some(module)
        || crate::builtins::modules::embedded_module_source(value::symbol_name_ref(module))
            .is_none()
    {
        return Ok(());
    }
    ensure_required(heap, env, module).map(|_| ())
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
    record_module(value::intern(module), value::intern(qualified_name));
}

/// Remember that `name` (a qualified reference) asks for `module`. The name is kept beside
/// the module so the eager drain can skip a module every one of whose recorded names is
/// ALREADY BOUND — `seq/lmap` is a prelude binding, and loading `std/seq.blsp` for it (and,
/// through its edges, `map`, `math`, `reflect` and `string`) added 63M instructions to
/// `brood --check` of a one-line file for a verdict that could not change (KI-150).
fn record_module(module: Symbol, name: Symbol) {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if !pending.iter().any(|(m, n)| *m == module && *n == name) {
            pending.push((module, name));
        }
    });
}

/// Record that the bare name `bare` — used in the form currently being resolved — is
/// `(:use …)`-imported from more than one module (`candidates`), so it cannot resolve
/// unambiguously (ADR-235). A no-op unless recording is armed (the read-only LSP resolve
/// must not raise). Pure — pushes to a thread-local, no heap/GC — so it is safe under
/// `resolve_sym`'s GC/macro blocks. Drained and raised by [`take_resolve_error`].
pub fn record_ambiguous(bare: Symbol, candidates: Vec<Symbol>) {
    if !RECORDING.with(|recording| recording.get()) {
        return;
    }
    PENDING_AMBIGUOUS.with(|pending| {
        let mut pending = pending.borrow_mut();
        if !pending.iter().any(|(b, _)| *b == bare) {
            pending.push((bare, candidates));
        }
    });
}

/// Record that a definition in `module` is headed by the bare `bare`, which names the dynamic
/// variable `target` there: a ROOT one (`target == bare`) or one `(:use …)`-imported from
/// another module (ADR-722), a `defdyn` head included. Until ADR-722 that spelling set the
/// variable, which was root;
/// now it would define `module/bare` and leave every reader of `target` unchanged, so it is
/// refused rather than reinterpreted (ADR-152). Same channel discipline as
/// [`record_ambiguous`]: armed only under `RECORDING`, raised by [`take_resolve_error`].
pub fn record_dynamic_def(bare: Symbol, target: Symbol, module: Symbol) {
    if RECORDING.with(|recording| recording.get()) {
        PENDING_DYNAMIC_DEF.with(|pending| {
            if pending.get().is_none() {
                pending.set(Some((bare, target, module)));
            }
        });
    }
}

/// The error the current resolve pass recorded, if any: a definition head that spells a
/// dynamic variable visible from elsewhere, then an ambiguous bare name (ADR-235). Called
/// right after `resolve` in the macroexpand driver, so the error points at the offending
/// form. Always clears both channels (so a caught error does not leak into the next pass).
pub fn take_resolve_error() -> Option<LispError> {
    let dynamic_def = PENDING_DYNAMIC_DEF.with(|pending| pending.take());
    let ambiguous = take_ambiguous_error();
    match dynamic_def {
        Some((bare, target, module)) if bare == target => {
            let bare = value::symbol_name_ref(bare);
            let module = value::symbol_name_ref(module);
            Some(LispError::runtime(format!(
                "`{bare}` is a root dynamic variable, so `{module}` cannot define its own \
                 `{bare}`: write `/{bare}` to set the root variable, or give the module's \
                 variable another name."
            )))
        }
        Some((bare, target, module)) => {
            let bare = value::symbol_name_ref(bare);
            let target = value::symbol_name_ref(target);
            let module = value::symbol_name_ref(module);
            Some(LispError::runtime(format!(
                "`{bare}` is `{target}` in `{module}`, a dynamic variable it imports: write \
                 `{target}` to set it, or `:exclude` it from the `(:use …)` to define the \
                 module's own."
            )))
        }
        None => ambiguous,
    }
}

/// If the current compile pass referenced any ambiguous bare name (ADR-235), take the
/// first and turn it into a use-site clash error naming the candidate modules; else `None`.
/// Always clears the channel. Candidates are sorted by their qualified spelling for a
/// stable message.
fn take_ambiguous_error() -> Option<LispError> {
    let first = PENDING_AMBIGUOUS.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending.is_empty() {
            return None;
        }
        let taken = std::mem::take(&mut *pending);
        taken.into_iter().next()
    });
    let (bare, mut candidates) = first?;
    candidates.sort_by(|a, b| value::symbol_name_ref(*a).cmp(value::symbol_name_ref(*b)));
    let names: Vec<String> = candidates
        .iter()
        .map(|q| format!("`{}`", value::symbol_name_ref(*q)))
        .collect();
    Some(LispError::runtime(format!(
        "`{}` is imported from more than one module ({}) — the bare name is ambiguous. \
         Qualify it (e.g. `{}`), or disambiguate the `(:use …)` with `:only [...]` / \
         `:exclude [...]` / an alias.",
        value::symbol_name_ref(bare),
        names.join(" and "),
        value::symbol_name_ref(candidates[0]),
    )))
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
                record_module(module, s);
            }
        }
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(form) else {
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
    // Lazy policy (ADR-335): an operand reference's module loads on first use, at the
    // lookup miss (`global_miss`), not here. The record is still taken so it cannot leak
    // into the next pass.
    if lazy_loads() {
        return Ok(Value::nil());
    }
    // A module is loaded only for a name that is NOT yet bound. A reference the prelude (or
    // an already-loaded module) satisfies — `seq/lmap`, `string/join`, `math/max` — asks
    // nothing of a load: the eager verdict for it is "bound" with or without `std/seq.blsp`
    // in the heap, and the unbound verdict for a genuine `json/prase` still gets `json`
    // loaded, because `json/prase` is unbound. What this removes is the load that could not
    // change the answer, which on a one-line `(seq/lmap …)` file was five modules and 63M
    // instructions of a 138M check (KI-150).
    //
    // A name the stdlib image carries a signature for (ADR-370) is deliberately NOT skipped
    // here: these are the FILE's own references, and the program is about to call them. The
    // pre-flight's load is the run's load brought forward; skipping it moved the load to
    // the first call, where a body compiled before the module arrived is recompiled
    // (ADR-366) — `base64`'s run read +37M instructions for a check that read −37M. What
    // the footer removes is the TRANSITIVE load — what a loaded module's bodies name
    // (`materialise_referenced_modules`), which the program never asked for.
    let global = heap.global();
    let mut modules: Vec<Symbol> = Vec::new();
    for (module, name) in &pending {
        if heap.env_get(global, *name).is_none() && !modules.contains(module) {
            modules.push(*module);
        }
    }
    if modules.is_empty() {
        return Ok(Value::nil());
    }
    // Each `ensure_required` loads a module, which collects. `env` may be a LOCAL
    // frame that the collector relocates, so root it and read it back per iteration
    // rather than holding a stale copy across the loads.
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(env);
    for module in modules {
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
    // A module we have already failed to FIND, with nothing loaded since: skip it. Resolving
    // a module name is a filesystem search, and the miss path runs it on every lookup of an
    // unbound qualified name — so `(mod/nope)` in a loop, which is what error-testing code
    // is, pays that search per iteration (~10x an unbound name in a loaded module).
    //
    // The memo is not permanent because a module absent now may exist later (ADR-335):
    // any successful load clears it (below), as does a load-path change
    // ([`clear_absent_modules`], called from `set-load-path`).
    if module_known_absent(heap, module) {
        return Ok(Value::nil());
    }
    let root = heap.env_root(env);
    // An INFERRED load records no require-edge (ADR-335): the edges exist so a module
    // materialised from an image gets the modules its source load would have pulled in, and
    // a body reference pulls nothing in — the miss path loads it on first use, imaged or
    // not. Only the header clauses (`:use`/`:alias`) are load-time edges. The
    // flag is one-shot and consumed by the loader's FIRST `%require-record-edge!` — the one
    // for `module` itself — so the edges of everything `module` in turn loads are kept.
    SKIP_NEXT_EDGE.with(|skip| skip.set(true));
    let loaded = crate::eval::compile::apply_engine(heap, loader, &[Value::symbol(module)], root);
    SKIP_NEXT_EDGE.with(|skip| skip.set(false)); // not consumed (no loader ran) — clear it
    match loaded {
        // The claim went to another process and this load was armed to hand off
        // (`parkable_miss`): nothing loaded, and the caller makes the wait a frame.
        Ok(Value::Keyword(k)) if value::symbol_is(k, "%hand-off") => {
            HANDED_OFF.with(|handed| handed.set(true));
            Ok(Value::nil())
        }
        Ok(value) => {
            // Something loaded, so the module set changed: a name that could not be found
            // before may be reachable now (the loaded module may add to the load path).
            clear_absent_modules(heap);
            Ok(value)
        }
        // Best-effort: inferring a require must not turn a reference into a compile error.
        // A module that cannot be found falls through to the normal handling — an in-file
        // module the checker knows without loading, or a genuine typo that surfaces as an
        // ordinary `unbound symbol: mod/name`. A real error *inside* a found module still
        // propagates, so a broken module is never silently hidden behind "unbound".
        Err(error) if error.message.contains("cannot find module") => {
            note_module_absent(heap, module);
            Ok(Value::nil())
        }
        // A transitive cycle: a module we auto-require refers back (qualified) into one
        // that is still loading. The reference is being satisfied by that in-progress
        // load itself, so inferring a re-require here must not become a hard error —
        // best-effort, same as an absent module. (The common self-reference case is
        // already filtered upstream in `record_qualified`.)
        Err(error) if error.message.contains("still loading") => Ok(Value::nil()),
        Err(error) => Err(error),
    }
}

// ===== Load policy + the miss-path autoload (ADR-335) =====================================
//
// A qualified reference is a promise that `mod/name` is bound WHEN THE REFERENCE IS
// EVALUATED. The hooks above infer the load at expansion time, which is right for a macro
// (it must expand now) and premature for a function: a dispatcher module loaded every
// subcommand's world to run one, and `(io/puts "hi")` materialised eight modules. Under the
// LAZY policy an operand reference only records its module, and the load happens on the
// global-lookup MISS path — `global_miss` — so a hit pays nothing. Under EAGER every
// recorded module loads at drain, as before: the checker needs that (its unbound verdict for
// `json/prase` depends on `json` being loaded), `nest run --check-boot` promises it, and
// `BROOD_NO_LAZY_LOAD=1` is the A/B and bisect lever.

/// Nesting depth of [`EagerLoadScope`]s — any open scope forces eager, process-wide. Eager
/// is always correct, so a concurrent lazy loader going eager for the duration is harmless.
static EAGER_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// The sticky process-wide switch behind `%eager-loads!` — for a tool whose whole run must
/// load eagerly (`nest run --check-boot`), where a Rust scope guard has no place to live.
static EAGER_STICKY: AtomicBool = AtomicBool::new(false);

/// Is the lazy policy in force right now? `BROOD_NO_LAZY_LOAD=1` pins the eager policy for
/// the whole process.
pub fn lazy_loads() -> bool {
    crate::diagnostics::debug_flags::lazy_load()
        && EAGER_DEPTH.load(Ordering::Relaxed) == 0
        && !EAGER_STICKY.load(Ordering::Relaxed)
}

/// Set the sticky eager switch (`%eager-loads!`); returns the previous setting.
pub fn set_eager_loads(on: bool) -> bool {
    EAGER_STICKY.swap(on, Ordering::Relaxed)
}

/// Scope under which every inferred load is EAGER (today's behaviour): the checker opens one
/// around its compile pass. Counted, so nested scopes compose.
pub struct EagerLoadScope;

impl EagerLoadScope {
    pub fn enter() -> Self {
        EAGER_DEPTH.fetch_add(1, Ordering::Relaxed);
        EagerLoadScope
    }
}

impl Drop for EagerLoadScope {
    fn drop(&mut self) {
        EAGER_DEPTH.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The module of an already-RESOLVED qualified name — the runtime counterpart of
/// [`module_to_require`], for [`global_miss`]. A name reaching a lookup has been through
/// `resolve_sym`: an alias prefix was rewritten to the real module path and an intra-package
/// name rooted, so the compile-time alias test is not applied here. It must not be: the
/// import table it reads is per-file compile state, and at run time it holds whatever the
/// last compiled file left — an alias `(:alias web/json :as json)` in some unrelated file
/// would otherwise make a plain `json/parse` miss read as "alias, already loaded" and raise
/// unbound. Nor is the name re-rooted: it already is, and `root_qualified_ref` reads the
/// package context, which during a dependency's load is that dependency's — a std `json/parse`
/// missing while a dependency with its own `json` module is mid-load would otherwise be sent
/// to `dep/json`. `/foo` (the root escape) and the bare `/` have no module.
pub(crate) fn module_of_resolved(s: Symbol) -> Option<Symbol> {
    let name = value::symbol_name_ref(s);
    let last = name.rfind('/')?;
    if last == 0 {
        return None;
    }
    Some(value::intern(&name[..last]))
}

/// The root binding a `/name` escape (ADR-236) names, read from the GLOBAL environment, or
/// `None` when `sym` is not an escape or `name` is unbound. The compile pass strips an escape
/// before code runs (renaming any local that would capture it); this is the escape's meaning
/// for code that never passed through it — a form a macro expanded lazily, a pattern lowered
/// at run time. No local can be named `/name`, so a lookup of one always misses its way here,
/// and reading only the global table keeps it immune to any local `name`.
pub(crate) fn root_escape_value(heap: &Heap, sym: Symbol) -> Option<Value> {
    let spelling = value::symbol_name_ref(sym);
    if spelling.len() < 2 || !spelling.starts_with('/') {
        return None;
    }
    let name = value::intern_existing(&spelling[1..])?;
    heap.env_get(EnvId::GLOBAL, name)
}

/// The global-lookup MISS path for every engine: a lookup of `sym` in `env` found nothing.
/// A *requireable* name — one with a module prefix, after the same exclusions
/// [`module_to_require`] applies — loads its module and is looked up again; only if it is
/// still unbound does the ordinary `unbound symbol` error follow. For a bare name this IS the
/// unbound error, so a caller's `None` arm is unchanged in cost and result.
///
/// Deliberately NOT gated on [`lazy_loads`]: the policy governs when the *compile pass*
/// loads, and this is the net under both. A module materialised from the stdlib image never
/// had a compile pass here — its body references were loaded at IMAGE-BUILD time, by the
/// builder's policy, and replayed as require-edges — so a lazily-built image needs this path
/// even in a process pinned eager, or `io`'s `file/spit-append` is an unbound error under
/// `BROOD_NO_LAZY_LOAD=1`.
///
/// The load is arbitrary evaluation and therefore a collection: `env` is rooted across it
/// here, but every OTHER handle the caller holds (an operand, a form, a cached `EnvId`) must
/// be rooted by the caller and re-read afterwards, exactly as across a call. The error rules
/// are [`ensure_required`]'s: a module that cannot be found, or a `still loading` cycle,
/// falls through to the plain unbound error; an error inside a found module propagates.
pub(crate) fn global_miss(heap: &mut Heap, env: EnvId, sym: Symbol) -> LispResult {
    if let Some(value) = root_escape_value(heap, sym) {
        return Ok(value);
    }
    // An UNCONTRACTED alias (ADR-383) with no binding: the arm was compiled while its
    // module's sibling held a contract shim, and the globals have since been rolled back
    // under it (an `%isolate` restore keeps the shared body cache). The public name is
    // the reference's meaning either way — it resolves to whatever the sibling holds
    // now, a shim or the plain closure, so a stale alias only ever checks more.
    if let Some(public) = crate::builtins::contracts::alias_public(sym) {
        return match heap.env_get(env, public) {
            Some(v) => Ok(v),
            None => global_miss(heap, env, public),
        };
    }
    if let Some(module) = module_of_resolved(sym) {
        // A module's reference to its OWN not-yet-defined name while it is mid-load:
        // never re-require it (mirrors `require_qualified_head`).
        if heap.compile_ns() != Some(module) {
            let env_base = heap.env_roots_len();
            let env_root = heap.root_env(env);
            let loaded = ensure_required(heap, env, module);
            let env = heap.read_root_env(env_root);
            heap.truncate_env_roots(env_base);
            loaded?;
            if let Some(v) = heap.env_get(env, sym) {
                return Ok(v);
            }
        }
    }
    Err(crate::eval::unbound_error(heap, sym))
}

/// The module of `sym` when ANOTHER live process is loading it right now — the one case in
/// which [`global_miss`] has to wait. That wait is a `receive` (`%require-await`), and from
/// here it would run under this native frame: a receive that cannot park, so it blocks its
/// OS worker. 200 processes making their first call into one module together dirty-blocked
/// 27 workers (2026-10-20). The VM sends such a miss through `%lazy-global` as an ordinary
/// call instead (`exec_chunk`), where the same wait parks. `None` for every other miss: a
/// loaded or absent module, this process's own load, a bare name.
pub(crate) fn module_loading_elsewhere(heap: &Heap, sym: Symbol) -> Option<Symbol> {
    let module = module_of_resolved(sym)?;
    if heap.compile_ns() == Some(module) {
        return None;
    }
    let name = value::symbol_name_ref(module);
    let loading = heap.env_get(
        heap.global(),
        value::intern(crate::core::registries::FEATURES_LOADING),
    )?;
    let Value::Map(id) = loading else {
        return None;
    };
    let me = crate::process::self_pid();
    let live = crate::process::list_local_pids();
    let elsewhere = heap.map_entries(id).into_iter().any(|(key, owner)| {
        let ours = match key.unpack() {
            ValueRef::Str(s) => heap.string(s) == name,
            _ => false,
        };
        ours && match owner {
            Value::Pid { node, id } if crate::dist::is_local(node) => {
                id != me && live.contains(&id)
            }
            _ => false,
        }
    });
    elsewhere.then_some(module)
}

/// The module of `sym` when [`global_miss`] would LOAD it — a module not yet provided, nor
/// known absent, nor the one being compiled. That load is arbitrary Brood under this native
/// frame, and every wait in it is a `receive` that cannot park, so it blocks its OS worker:
/// the processes that lose the race to claim the load wait for the winner (200 making their
/// first call into one module together dirty-blocked up to 27 workers, 2026-10-20), and the
/// winner waits on any dependency another process is mid-load of (139 blocks for 40 loaders
/// of modules whose header `(:use http)` while `http` loaded elsewhere). The VM sends such a
/// miss through `%lazy-global` / `%lazy-call` as an ordinary call instead (`exec_chunk`),
/// where the same waits park. `None` for every other miss — a loaded module's unbound name, a
/// bare name — which the native path answers with the unbound error at no extra cost.
pub(crate) fn module_to_load_off_native(heap: &mut Heap, sym: Symbol) -> Option<Symbol> {
    let module = module_of_resolved(sym)?;
    if heap.compile_ns() == Some(module) || module_known_absent(heap, module) {
        return None;
    }
    let name = heap.alloc_string(value::symbol_name_ref(module));
    let provided = heap.registry_member(value::intern(crate::core::registries::FEATURES), name);
    (!provided).then_some(module)
}

/// What a miss in a frame that can park resolved to ([`parkable_miss`]).
pub(crate) enum ParkableMiss {
    /// The name's value, loaded by this miss if it had to be.
    Bound(Value),
    /// Another process holds the load of this module: the caller hands the wait to the
    /// prelude's `%lazy-global`/`%lazy-call` as frames, where it parks.
    Elsewhere(Symbol),
}

/// [`global_miss`] for a frame that can park, with the load's claim race closed. Checking
/// [`module_loading_elsewhere`] first is not enough on its own: another process can take the
/// claim between that check and `require-one`'s own, and the loser then waits in
/// `%require-await`'s `receive` under this native lookup — blocking its worker, which is
/// what the hand-off exists to avoid. So the load is armed: a `require-one` that loses the
/// claim answers `:%hand-off` without waiting, and the miss reports `Elsewhere` as if the
/// check had seen the claim. A load that wins runs here as before. `arm` is the compiled
/// arm making the miss, marked stale when the miss loads (as [`global_miss_in_arm`]).
pub(crate) fn parkable_miss(
    heap: &mut Heap,
    env: EnvId,
    sym: Symbol,
    arm: Option<&crate::eval::compile::CompiledArm>,
) -> Result<ParkableMiss, LispError> {
    if let Some(module) = module_loading_elsewhere(heap, sym) {
        return Ok(ParkableMiss::Elsewhere(module));
    }
    claim_race_fault(sym);
    HAND_OFF_ARMED.with(|armed| armed.set(true));
    let looked_up = match arm {
        Some(arm) => global_miss_in_arm(heap, env, sym, arm),
        None => global_miss(heap, env, sym),
    };
    // Not consumed when no `require-one` ran (a bare name, an absent module) — clear it.
    HAND_OFF_ARMED.with(|armed| armed.set(false));
    if HANDED_OFF.with(|handed| handed.replace(false))
        && let Some(module) = module_of_resolved(sym)
    {
        return Ok(ParkableMiss::Elsewhere(module));
    }
    looked_up.map(ParkableMiss::Bound)
}

/// Fault injection for the claim race (`BROOD_FAULT_CLAIM_RACE=<module>`): the runtime's FIRST
/// parkable miss into `<module>` sleeps 500 ms after deciding to load and before
/// `require-one`'s claim — in [`parkable_miss`], after the loading-elsewhere check, and on the
/// VM's route for a module not yet provided (`exec_chunk::lazy_target`) — so a test can have
/// another process take the claim inside the window. One-shot, so the process that takes it
/// does not pause too.
pub(crate) fn claim_race_fault(sym: Symbol) {
    static TAKEN: AtomicBool = AtomicBool::new(false);
    let Some(target) = std::env::var_os("BROOD_FAULT_CLAIM_RACE") else {
        return;
    };
    let ours =
        module_of_resolved(sym).is_some_and(|m| target.to_str() == Some(value::symbol_name_ref(m)));
    if !ours || TAKEN.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
}

/// [`global_miss`] from inside a compiled arm's execution (ADR-366). If the miss actually
/// LOADED a module — the global epoch moved across it — the arm's body was compiled without
/// that module's bindings, so its compile-time decisions are stale: mark it, and the cache
/// lookups recompile it at its next activation (see `CompiledArm::stale_bindings`). A miss
/// that loaded nothing (a plain unbound name, an absent module) marks nothing.
pub(crate) fn global_miss_in_arm(
    heap: &mut Heap,
    env: EnvId,
    sym: Symbol,
    arm: &crate::eval::compile::CompiledArm,
) -> LispResult {
    let epoch = heap.global_epoch();
    let v = global_miss(heap, env, sym)?;
    if heap.global_epoch() != epoch {
        mark_arm_stale_for_miss(heap, arm, sym);
    }
    Ok(v)
}

/// Mark `arm` stale because a miss on `sym` loads its module, and say so under
/// `BROOD_TRACE_COMPILE` — the native path above and the VM's `%lazy-global` / `%lazy-call`
/// route (`exec_chunk`) alike, so the trace shows every first-use load whichever ran it.
pub(crate) fn mark_arm_stale_for_miss(
    heap: &mut Heap,
    arm: &crate::eval::compile::CompiledArm,
    sym: Symbol,
) {
    heap.mark_arm_stale(arm);
    if crate::diagnostics::debug_flags::trace_compile() {
        let name = arm.dbg_label();
        eprintln!(
            "[compile] stale-bindings arm={name}: a miss on {} loaded its module; recompiles at its next activation",
            value::symbol_name_ref(sym)
        );
    }
}

// ===== The image's kind index: which qualified HEADS may defer (ADR-335 item 2) ===========
//
// A qualified call head into an unloaded module must load NOW if it might be a macro — a
// macro expands at compile time — and whether it is one is unknowable without loading,
// except that a startup image records every binding's kind. `%image-index` registers, for
// each image it opens, the modules it holds and the macro names among them (the v6 footer),
// so a head into an imaged module whose name is not a recorded macro is a FUNCTION and may
// wait for its first call. A head into an un-imaged module, or to a recorded macro, loads
// eagerly as it always has. An image is fingerprint-rejected before it reaches this, so a
// stale kind cannot be registered. The index is the RUNTIME's (`Heap::register_image_kinds`):
// another runtime in the process may have opened a different image, or none (KI-226).

/// Record the modules (section names) and macro names an opened image holds. Additive: the
/// stdlib image and a project image both register, and nothing un-registers — a module that
/// later loads from source is simply bound, which every check here tests first.
pub fn register_image_kinds(heap: &Heap, modules: &[String], macros: &[String]) {
    heap.register_image_kinds(
        modules
            .iter()
            .filter(|module| !module.is_empty())
            .map(|module| value::intern(module)),
        macros.iter().map(|name| value::intern(name)),
    );
}

// ===== The image's signature index (ADR-370) ===============================================
//
// The v7 footer carries, for every imaged module's function, the signature the checker
// derives for it at image-build time — declared or inferred, with every module loaded. Read at
// open as TEXT (the footer is read before any heap work) and kept here; `sigs::image_heap_sig`
// parses one on demand, into a process-wide cache, so a check pays only for the callees it
// meets and a run that never checks pays nothing. The module set is what
// `materialise_referenced_modules` and the image's edge replay consult: a module the image
// has signatures for is never loaded for the sake of a body walk.

struct ImageSigs {
    /// `name -> (type-form text, min arity, max arity or u32::MAX)`.
    texts: std::collections::HashMap<Symbol, (String, u32, u32)>,
    /// Filled whole — from an image's footer at open, or by `image_sig_entries` scanning
    /// every embedded source in a process that has no image.
    filled: bool,
    /// Modules a process with no image has scanned on demand (`ensure_module_indexed`).
    scanned: std::collections::HashSet<String>,
    /// An opened image's index, still as footer bytes: decoded by the first query, so a
    /// boot that never checks never pays for it.
    raw: Option<Vec<u8>>,
}

fn image_sigs() -> &'static std::sync::RwLock<ImageSigs> {
    static SIGS: std::sync::OnceLock<std::sync::RwLock<ImageSigs>> = std::sync::OnceLock::new();
    SIGS.get_or_init(|| {
        std::sync::RwLock::new(ImageSigs {
            texts: Default::default(),
            filled: false,
            scanned: Default::default(),
            raw: None,
        })
    })
}

/// `BROOD_NO_IMAGE_SIGS=1` — opt OUT of the std signature index (ADR-370): the checker
/// infers loaded bodies and materialises what they name, as before. The A/B, bisect and
/// differential lever. Cached once.
pub fn image_sigs_enabled() -> bool {
    crate::diagnostics::debug_flags::image_signatures()
}

/// Record the signature index an opened image holds — `(qualified name, type text, min
/// arity, max arity)` per function. Additive like the kind index.
pub fn register_image_sigs(entries: Vec<(String, String, u32, u32)>) {
    if crate::diagnostics::debug_flags::image_trace() {
        let typed = entries
            .iter()
            .filter(|(_, text, _, _)| !text.is_empty())
            .count();
        eprintln!(
            "[image] signature index: {} names, {typed} with an authoritative type",
            entries.len()
        );
    }
    let mut sigs = image_sigs().write().unwrap_or_else(|e| e.into_inner());
    sigs.filled = true;
    for (name, text, min, max) in entries {
        sigs.texts.insert(value::intern(&name), (text, min, max));
    }
}

/// Set aside an opened image's index as the footer bytes it came in (`image::decode_sig_index`
/// reads them on the first query). Replaces an earlier image's bytes; a later `stdimage/build`
/// in the same process re-registers.
pub fn register_image_sigs_raw(bytes: Vec<u8>) {
    let mut sigs = image_sigs().write().unwrap_or_else(|e| e.into_inner());
    sigs.raw = Some(bytes);
}

/// Decode the set-aside footer bytes into the index, once — the first query pays it.
fn settle_raw() {
    // Every signature query passes here, and after the first nothing is left to settle: a
    // shared read answers that, where taking the write lock serialised every query.
    if image_sigs()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .raw
        .is_none()
    {
        return;
    }
    let raw = {
        let mut sigs = image_sigs().write().unwrap_or_else(|e| e.into_inner());
        sigs.raw.take()
    };
    if let Some(bytes) = raw {
        register_image_sigs(crate::boot::image::decode_sig_index(bytes));
    }
}

/// The index is a fact about std's SOURCE; the image only caches it. A process that booted
/// without an image computes it here from the sources baked into the binary — ONE module at
/// a time, the module a question is about, so a source-boot check pays for the modules it
/// meets — and so the checker knows the same about a std callee whether or not
/// `~/.cache/brood` holds an image (the artifact matrix read a prelude function's inferred
/// signature differently in the two cells before this). On a scratch heap sharing this
/// one's regions, like `image_heap_sig`.
fn ensure_module_indexed(heap: &Heap, sym: Symbol) {
    settle_raw();
    let name = value::symbol_name_ref(sym);
    let Some(slash) = name.rfind('/') else {
        return;
    };
    let key = &name[..slash];
    {
        let sigs = image_sigs().read().unwrap_or_else(|e| e.into_inner());
        if sigs.filled || sigs.scanned.contains(key) {
            return;
        }
    }
    let Some(source) = crate::builtins::modules::embedded_module_source(key) else {
        return; // not a std module — nothing to index
    };
    let mut scratch = Heap::with_regions(heap.prelude_arc(), heap.runtime_arc());
    scratch.set_global(EnvId::GLOBAL);
    let entries = crate::types::check::module_signature_index(&mut scratch, key, source);
    if crate::diagnostics::debug_flags::image_trace() {
        eprintln!(
            "[image] signature index: no image — {key} scanned from its embedded source ({} names)",
            entries.len()
        );
    }
    let mut sigs = image_sigs().write().unwrap_or_else(|e| e.into_inner());
    sigs.scanned.insert(key.to_string());
    for e in entries {
        sigs.texts
            .entry(value::intern(&e.name))
            .or_insert((e.text, e.min, e.max));
    }
}

/// The whole index, for a process with no image: every embedded source scanned once.
fn ensure_std_index(heap: &Heap) {
    settle_raw();
    if image_sigs()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .filled
    {
        return;
    }
    let mut scratch = Heap::with_regions(heap.prelude_arc(), heap.runtime_arc());
    scratch.set_global(EnvId::GLOBAL);
    let entries: Vec<(String, String, u32, u32)> =
        crate::types::check::std_signature_index(&mut scratch)
            .into_iter()
            .map(|e| (e.name, e.text, e.min, e.max))
            .collect();
    if crate::diagnostics::debug_flags::image_trace() {
        eprintln!("[image] signature index: no image — computed from the embedded sources");
    }
    register_image_sigs(entries);
}

/// The index's recorded arity for `sym` — `(min, max)`, `None` max for a rest parameter.
pub fn image_sig_arity(heap: &Heap, sym: Symbol) -> Option<(usize, Option<usize>)> {
    if !image_sigs_enabled() {
        return None;
    }
    ensure_module_indexed(heap, sym);
    image_sigs()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .texts
        .get(&sym)
        .map(|(_, min, max)| (*min as usize, (*max != u32::MAX).then_some(*max as usize)))
}

/// The index's AUTHORITATIVE signature text for `sym`, if it has one — an undeclared
/// function has an entry (existence, arity) but no text, and answers `None` here so its
/// callers keep loading the module and inferring the body.
pub fn image_sig_text(heap: &Heap, sym: Symbol) -> Option<String> {
    if !image_sigs_enabled() {
        return None; // the lever: every consumer falls back to loading and inferring
    }
    ensure_module_indexed(heap, sym);
    image_sigs()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .texts
        .get(&sym)
        .filter(|(text, _, _)| !text.is_empty())
        .map(|(text, _, _)| text.clone())
}

/// Every indexed name that carries an authoritative type (ADR-370), sorted — the
/// construction gate walks these against the loaded modules' declarations. Empty when
/// disabled.
pub fn image_sig_typed_names(heap: &Heap) -> Vec<Symbol> {
    image_sig_entries(heap)
        .into_iter()
        .filter(|(_, text, _, _)| !text.is_empty())
        .map(|(name, _, _, _)| value::intern(&name))
        .collect()
}

/// The whole index as the footer spells it, sorted by name. Empty when disabled.
pub fn image_sig_entries(heap: &Heap) -> Vec<(String, String, u32, u32)> {
    if !image_sigs_enabled() {
        return Vec::new();
    }
    ensure_std_index(heap);
    let mut out: Vec<(String, String, u32, u32)> = image_sigs()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .texts
        .iter()
        .map(|(sym, (text, min, max))| (value::symbol_name(*sym), text.clone(), *min, *max))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod kind_index_tests {
    use super::*;

    /// Every macro a std module defines once LOADED, and the names its SOURCE scan reports,
    /// as sorted text. Read from a runtime that loaded every embedded module.
    fn loaded_and_scanned_macros() -> (Vec<String>, Vec<String>) {
        let mut interp = crate::Interp::new();
        let macros_now = "(%string-join \" \" (sort (map (seq/filter (reflect/global-names) \
             (fn (s) (try (= :macro (type-of (reflect/eval s))) (catch _ false)))) ->string)))";
        let read = |interp: &mut crate::Interp| -> Vec<String> {
            let v = interp.eval_str(macros_now).expect("list the macros");
            let Value::Str(id) = v else {
                panic!("the macro listing is not a string")
            };
            interp
                .heap
                .string(id)
                .split_whitespace()
                .map(str::to_string)
                .collect()
        };
        let before = read(&mut interp);
        let mut scanned: Vec<String> = Vec::new();
        for (key, src) in crate::builtins::modules::embedded_modules() {
            // A module this build cannot load (a feature it was built without) defines
            // nothing here, so its scan is left out as well.
            if interp.eval_str(&format!("(require-one '{key})")).is_err() {
                continue;
            }
            scanned.extend(
                embedded_macro_names(key, src)
                    .into_iter()
                    .map(value::symbol_name),
            );
        }
        let mut loaded: Vec<String> = read(&mut interp)
            .into_iter()
            .filter(|name| !before.contains(name))
            .collect();
        loaded.sort();
        loaded.dedup();
        scanned.sort();
        scanned.dedup();
        (loaded, scanned)
    }

    /// The source boot's kind index (`index_embedded_kinds`) reads a std module's macros off
    /// its top-level `defmacro` forms. A macro that arrives any other way (made by another
    /// macro, defined inside a `do` or a `when`) would be deferred past expansion as though
    /// it were a function, and its call would expand as a function call. Loading every std
    /// module and comparing is what keeps the scan honest.
    #[test]
    fn std_macro_kinds_match_the_loaded_modules() {
        let (loaded, scanned) = loaded_and_scanned_macros();
        assert!(
            !loaded.is_empty(),
            "loading every std module defined no macro: the comparison proves nothing"
        );
        let missed: Vec<&String> = loaded.iter().filter(|m| !scanned.contains(m)).collect();
        let extra: Vec<&String> = scanned.iter().filter(|m| !loaded.contains(m)).collect();
        assert!(
            missed.is_empty() && extra.is_empty(),
            "the source scan disagrees with the loaded std modules. Macros it does not see \
             (define them with a top-level `defmacro`, or teach `embedded_macro_names` the \
             form): {missed:?}. Names it calls macros that are not: {extra:?}"
        );
    }
}
