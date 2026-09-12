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
//! ([`EagerLoadScope`], `%eager-loads!`, `BROOD_NO_LAZY_LOAD=1`) the hooks load as they
//! always did; the checker runs eager so its unbound verdict sees the modules a file names.
//! Either way an inferred load records no require-edge ([`take_skip_next_edge`]): the image
//! replays edges so a materialised module has what its source load pulled in, and a body
//! reference no longer pulls anything in.
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
    static PENDING: RefCell<Vec<Symbol>> = const { RefCell::new(Vec::new()) };
    /// Bare names the current compile pass used that are `(:use …)`-imported from two or
    /// more modules at once (ADR-235): `(bare, sorted candidate qualifieds)`. Recorded by
    /// `resolve_sym` at the point the ambiguous name is referenced, raised as a use-site
    /// error by [`take_ambiguous_error`]. Only armed under `RECORDING` (the read-only LSP
    /// path must not hard-error a hover).
    static PENDING_AMBIGUOUS: RefCell<Vec<(Symbol, Vec<Symbol>)>> =
        const { RefCell::new(Vec::new()) };
    /// Set by [`ensure_required`] for the load it is about to run, consumed by the loader's
    /// next `%require-record-edge!` (via [`take_skip_next_edge`]): an inferred load is not a
    /// load-time edge. One-shot, so the edges of the loaded module's OWN header clauses are
    /// recorded as usual.
    static SKIP_NEXT_EDGE: Cell<bool> = const { Cell::new(false) };
}

/// Consume the one-shot "this load is inferred, record no edge" flag. Called by the
/// `%load-edge-skipped?` primitive at the top of `require-one`.
pub fn take_skip_next_edge() -> bool {
    SKIP_NEXT_EDGE.with(|skip| skip.replace(false))
}

/// Modules a require has already failed to FIND, with nothing loaded since — see
/// [`ensure_required`], which is the only reader. Process-wide rather than per-process
/// because module resolution reads the filesystem and the load path, both of which are
/// too; a `RwLock` because the common access is the read on the miss path.
static ABSENT_MODULES: std::sync::OnceLock<std::sync::RwLock<std::collections::HashSet<Symbol>>> =
    std::sync::OnceLock::new();

fn absent_modules() -> &'static std::sync::RwLock<std::collections::HashSet<Symbol>> {
    ABSENT_MODULES.get_or_init(Default::default)
}

fn module_known_absent(module: Symbol) -> bool {
    absent_modules()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&module)
}

fn note_module_absent(module: Symbol) {
    absent_modules()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(module);
}

/// Forget every recorded absence. Called when the inputs to module resolution change: a
/// successful load (in [`ensure_required`]) and a load-path change (`reflect/set-load-path`).
/// Clearing wholesale rather than per-module is deliberate — a new load path can make any
/// number of previously-absent modules resolvable, and the set is small.
pub fn clear_absent_modules() {
    absent_modules()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
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
    // Lazy policy (ADR-335): a head an image vouches is a FUNCTION waits for its first call
    // — the miss path (`global_miss`) loads it then. Only a macro, or a head into a module
    // no image describes, must load before expansion.
    if lazy_loads() && image_says_function(module, rooted) {
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

/// Record that the bare name `bare` — used in the form currently being resolved — is
/// `(:use …)`-imported from more than one module (`candidates`), so it cannot resolve
/// unambiguously (ADR-235). A no-op unless recording is armed (the read-only LSP resolve
/// must not raise). Pure — pushes to a thread-local, no heap/GC — so it is safe under
/// `resolve_sym`'s GC/macro blocks. Drained and raised by [`take_ambiguous_error`].
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

/// If the current compile pass referenced any ambiguous bare name (ADR-235), take the
/// first and turn it into a use-site clash error naming the candidate modules; else `None`.
/// Called right after `resolve` in the macroexpand driver, so the error points at the form
/// that used the name. Always clears the channel (so a caught error does not leak into the
/// next pass). Candidates are sorted by their qualified spelling for a stable message.
pub fn take_ambiguous_error() -> Option<LispError> {
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
    // Lazy policy (ADR-335): an operand reference's module loads on first use, at the
    // lookup miss (`global_miss`), not here. The record is still taken so it cannot leak
    // into the next pass.
    if lazy_loads() {
        return Ok(Value::nil());
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
    // A module we have already failed to FIND, with nothing loaded since: skip it. Resolving
    // a module name is a filesystem search, and the miss path runs it on every lookup of an
    // unbound qualified name — so `(mod/nope)` in a loop, which is what error-testing code
    // is, pays that search per iteration. Measured under the tree-walker, 2000 lookups:
    // 1.22 s for an absent module against 0.07 s for an unbound name in a module that IS
    // loaded (there `require-one` short-circuits on `*features*`) and 0.12 s for an unbound
    // bare name. That 10x is what turned the tree-walker suite job red.
    //
    // ADR-335 declined a negative memo because "a module absent now may exist later", which
    // is true and is why this one is not permanent: any successful load clears it (below),
    // as does a load-path change ([`clear_absent_modules`], called from `set-load-path`).
    // What it removes is only the repeat of a search whose inputs have not changed since it
    // failed.
    if module_known_absent(module) {
        return Ok(Value::nil());
    }
    let root = heap.env_root(env);
    // An INFERRED load records no require-edge (ADR-335): the edges exist so a module
    // materialised from an image gets the modules its source load would have pulled in, and
    // a body reference no longer pulls anything in — the miss path loads it on first use,
    // imaged or not. Only the header clauses (`:use`/`:alias`) are load-time edges now. The
    // flag is one-shot and consumed by the loader's FIRST `%require-record-edge!` — the one
    // for `module` itself — so the edges of everything `module` in turn loads are kept.
    SKIP_NEXT_EDGE.with(|skip| skip.set(true));
    let loaded = crate::eval::compile::apply_engine(heap, loader, &[Value::symbol(module)], root);
    SKIP_NEXT_EDGE.with(|skip| skip.set(false)); // not consumed (no loader ran) — clear it
    match loaded {
        Ok(value) => {
            // Something loaded, so the module set changed: a name that could not be found
            // before may be reachable now (the loaded module may add to the load path).
            clear_absent_modules();
            Ok(value)
        }
        // Best-effort: inferring a require must not turn a reference into a compile error.
        // A module that cannot be found falls through to the normal handling — an in-file
        // module the checker knows without loading, or a genuine typo that surfaces as an
        // ordinary `unbound symbol: mod/name`. A real error *inside* a found module still
        // propagates, so a broken module is never silently hidden behind "unbound".
        Err(error) if error.message.contains("cannot find module") => {
            note_module_absent(module);
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

/// `BROOD_NO_LAZY_LOAD=1` — pin the eager policy for the whole process. One cached read.
fn lazy_by_env() -> bool {
    use std::sync::OnceLock;
    static LAZY: OnceLock<bool> = OnceLock::new();
    *LAZY.get_or_init(|| std::env::var_os("BROOD_NO_LAZY_LOAD").is_none())
}

/// Nesting depth of [`EagerLoadScope`]s — any open scope forces eager, process-wide. Eager
/// is always correct, so a concurrent lazy loader going eager for the duration is harmless.
static EAGER_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// The sticky process-wide switch behind `%eager-loads!` — for a tool whose whole run must
/// load eagerly (`nest run --check-boot`), where a Rust scope guard has no place to live.
static EAGER_STICKY: AtomicBool = AtomicBool::new(false);

/// Is the lazy policy in force right now?
pub fn lazy_loads() -> bool {
    lazy_by_env()
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
fn module_of_resolved(s: Symbol) -> Option<Symbol> {
    let name = value::symbol_name_ref(s);
    let last = name.rfind('/')?;
    if last == 0 {
        return None;
    }
    Some(value::intern(&name[..last]))
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

// ===== The image's kind index: which qualified HEADS may defer (ADR-335 item 2) ===========
//
// A qualified call head into an unloaded module must load NOW if it might be a macro — a
// macro expands at compile time — and whether it is one is unknowable without loading,
// except that a startup image records every binding's kind. `%image-index` registers, for
// each image it opens, the modules it holds and the macro names among them (the v6 footer),
// so a head into an imaged module whose name is not a recorded macro is a FUNCTION and may
// wait for its first call. A head into an un-imaged module, or to a recorded macro, loads
// eagerly as it always has. An image is fingerprint-rejected before it reaches this, so a
// stale kind cannot be registered.

struct ImageKinds {
    modules: std::collections::HashSet<Symbol>,
    macros: std::collections::HashSet<Symbol>,
}

fn image_kinds() -> &'static std::sync::RwLock<ImageKinds> {
    static KINDS: std::sync::OnceLock<std::sync::RwLock<ImageKinds>> = std::sync::OnceLock::new();
    KINDS.get_or_init(|| {
        std::sync::RwLock::new(ImageKinds {
            modules: Default::default(),
            macros: Default::default(),
        })
    })
}

/// Record the modules (section names) and macro names an opened image holds. Additive: the
/// stdlib image and a project image both register, and nothing un-registers — a module that
/// later loads from source is simply bound, which every check here tests first.
pub fn register_image_kinds(modules: &[String], macros: &[String]) {
    let mut kinds = image_kinds().write().unwrap_or_else(|e| e.into_inner());
    for module in modules {
        if !module.is_empty() {
            kinds.modules.insert(value::intern(module));
        }
    }
    for name in macros {
        kinds.macros.insert(value::intern(name));
    }
}

/// Does an opened image vouch that `qualified` (in `module`) is NOT a macro? True only for
/// a module some image holds whose recorded macros do not include the name.
pub fn image_says_function(module: Symbol, qualified: Symbol) -> bool {
    let kinds = image_kinds().read().unwrap_or_else(|e| e.into_inner());
    kinds.modules.contains(&module) && !kinds.macros.contains(&qualified)
}
