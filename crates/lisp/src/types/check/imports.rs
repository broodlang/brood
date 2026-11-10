//! A file's imports and requires as the checker reads them: the `(defmodule …)` header's
//! `:use`/`:alias`/`:load` clauses, every `(require …)` target, the file's world closed
//! through the standard library, and the import table set up from the header without
//! evaluating it ([`ImportScope`]).

use super::*;

/// A namespace header — `(defmodule …)` (checked on the *un-expanded* form, before
/// its `(:use …)` clauses lower away). The checker evaluates it so the header's
/// `(require …)`/`%refer`/`%in-ns` run — populating the import table — and a
/// `(:use …)`-imported name then resolves instead of looking unbound.
pub(super) fn is_ns_header(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, kw::DEFMODULE);
        }
    }
    false
}

/// Parse the `(defmodule … (:use mod) …)` header from the *unexpanded* `forms`
/// and return the module names explicitly listed in `:use` clauses.
pub(super) fn extract_use_module_names(heap: &Heap, forms: &[Value]) -> Vec<String> {
    extract_clause_modules(heap, forms, &["use"])
}

/// Every module named in an *import* clause of the header — `(:use mod)`,
/// `(:use-internals mod)`, and `(:alias mod [:as short])`, each of which **loads** `mod`
/// (an `:alias` `(require …)`s its target, then adds the `short/` prefix; references via
/// the alias macro-expand to `mod/…`). Used for the file's world (B8) and the
/// whole-project require edges — a module reached by any of these is genuinely required. Kept separate from
/// [`extract_use_module_names`], whose `:use`-only view backs the unused-import lint.
fn extract_import_module_names(heap: &Heap, forms: &[Value]) -> Vec<String> {
    extract_clause_modules(heap, forms, &["use", "use-internals", "alias", "load"])
}

/// The module names listed by any header clause whose keyword is in `keywords`
/// (e.g. `:use` / `:use-internals`), read from the *unexpanded* `(defmodule …)` form.
fn extract_clause_modules(heap: &Heap, forms: &[Value], keywords: &[&str]) -> Vec<String> {
    for &form in forms {
        if !is_ns_header(heap, form) {
            continue;
        }
        // (defmodule name clause...)
        let Value::Pair(p) = form else { continue };
        let (_, rest) = heap.pair(p);
        let Value::Pair(r) = rest else { continue };
        let (_, clauses) = heap.pair(r); // skip the module name

        let mut result = Vec::new();
        let mut cur = clauses;
        while let Value::Pair(p) = cur {
            let (clause, next) = heap.pair(p);
            // Each (:use mod …) / (:use-internals mod …) clause starts with its keyword.
            if let Some(items) = list_items(heap, clause)
                && let Some(Value::Keyword(kw_sym)) = items.first()
                && keywords.iter().any(|k| value::symbol_is(*kw_sym, k))
            {
                if value::symbol_is(*kw_sym, "load") {
                    // `(:load a b …)` names several modules (ADR-335); every symbol
                    // after the keyword is one. The other clauses take one module
                    // and may carry a symbol that is NOT a module (`:alias m :as short`).
                    for item in items.iter().skip(1) {
                        if let Value::Sym(mod_sym) = item {
                            result.push(value::symbol_name(*mod_sym).to_string());
                        }
                    }
                } else if let Some(&Value::Sym(mod_sym)) = items.get(1) {
                    result.push(value::symbol_name(mod_sym).to_string());
                }
            }
            cur = next;
        }
        return result;
    }
    Vec::new()
}

/// The module name a top-level `(require …)` targets, if any — `(require 'mod)`,
/// `(require "mod")`, or `(require 'mod :as m)`. `None` for a non-require form or a
/// dynamic require whose module argument isn't a literal symbol/string.
fn require_target(heap: &Heap, form: Value) -> Option<String> {
    let items = list_items(heap, form)?;
    match items.first() {
        Some(&Value::Sym(h)) if value::symbol_is(h, "require-one") => {}
        _ => return None,
    }
    match *items.get(1)? {
        // `'mod` reads as `(quote mod)`.
        arg @ Value::Pair(_) => {
            let inner = list_items(heap, arg)?;
            match (inner.first(), inner.get(1)) {
                (Some(&Value::Sym(q)), Some(&Value::Sym(m))) if value::symbol_is(q, "quote") => {
                    Some(value::symbol_name(m))
                }
                _ => None,
            }
        }
        Value::Sym(m) => Some(value::symbol_name(m)),
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    }
}

/// The module prefixes the file makes reachable *itself*: every `(:use M)` in its
/// header, every `(require 'M)` **anywhere** in the file (including one nested in a
/// function body — `project.blsp` requires `test`/`package` lazily inside the functions
/// that use them, to keep startup lean), and its own namespace. A file that requires `M`
/// *somewhere* is treated as reaching `M` for the whole file — an over-approximation.
/// Direct requires only, not their transitive closure. The seed of the file's world
/// (B8, `check_file_mode`).
pub(super) fn collect_required_modules(
    heap: &Heap,
    forms: &[Value],
    file_ns: Option<Symbol>,
) -> HashSet<String> {
    let mut mods: HashSet<String> = extract_import_module_names(heap, forms)
        .into_iter()
        .collect();
    if let Some(ns) = file_ns {
        mods.insert(value::symbol_name(ns));
    }
    // Every module the file itself declares (ADR-223: a file may open more than one
    // `(defmodule …)`) is trivially reachable — an intra-file qualified reference to a
    // co-located sibling module is in the file's world. `file_ns` above
    // is only the first; add the rest (the caller roots every entry, so the bare names here
    // gain their rooted form too).
    for m in crate::eval::macros::file_modules(heap, forms) {
        mods.insert(value::symbol_name(m));
    }
    for &form in forms {
        collect_require_targets(heap, form, &mut mods);
    }
    mods
}

/// Extend `mods` with every module reachable from it through the standard library's own
/// require edges (B8). A baked-in module's direct requires are read from its
/// embedded source once per process (`STD_REQUIRES`); a name that is not a baked-in module
/// is a leaf here (the whole-project driver has already closed the project's own edges).
pub(super) fn close_through_std(heap: &mut Heap, mods: &mut HashSet<String>) {
    static STD_REQUIRES: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<String>>>> =
        std::sync::OnceLock::new();
    let cache = STD_REQUIRES.get_or_init(Default::default);
    let mut frontier: Vec<String> = mods.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        let cached = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&name)
            .cloned();
        let direct = match cached {
            Some(direct) => direct,
            None => {
                let direct = match crate::builtins::modules::embedded_module_source(&name) {
                    Some(source) => crate::syntax::reader::read_all(heap, source)
                        .map(|forms| module_direct_requires(heap, &forms).1)
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(name.clone(), direct.clone());
                direct
            }
        };
        for dep in direct {
            if mods.insert(dep.clone()) {
                frontier.push(dep);
            }
        }
    }
}

/// A file's own module name (from its `(defmodule …)` header, or `None`) and the module
/// names it **directly** pulls in — `(:use …)` / `(:use-internals …)` clauses plus every
/// `(require 'M)` anywhere in the file. The edge list the whole-project driver
/// (`std/tool/project.blsp`) closes transitively into each file's require-closure, then
/// feeds back to [`check_file_ext`] as the seed of the file's world (B8). Own ns is excluded from `requires`.
pub fn module_direct_requires(heap: &Heap, forms: &[Value]) -> (Option<String>, Vec<String>) {
    let file_ns = crate::eval::macros::file_ns(heap, forms);
    let own = file_ns.map(value::symbol_name);
    let mut deps: HashSet<String> = extract_import_module_names(heap, forms)
        .into_iter()
        .collect();
    for &form in forms {
        collect_require_targets(heap, form, &mut deps);
    }
    if let Some(ref n) = own {
        deps.remove(n);
    }
    let mut deps: Vec<String> = deps.into_iter().collect();
    deps.sort(); // deterministic order (no Date/random in the checker)
    (own, deps)
}

/// Add every `(require 'M)` target reachable in `form` — descending through list,
/// vector, and map structure so a `require` nested in any function body is found.
fn collect_require_targets(heap: &Heap, form: Value, out: &mut HashSet<String>) {
    // Guard the deep-form recursion the same way `count_defs` does — a pathologically
    // nested source must not overflow the checker's stack (tests/…deep_forms).
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        if let Some(m) = require_target(heap, form) {
            out.insert(m);
        }
        match form {
            Value::Pair(pid) => {
                let (car, cdr) = heap.pair(pid);
                collect_require_targets(heap, car, out);
                collect_require_targets(heap, cdr, out);
            }
            Value::Vector(vid) => {
                for v in heap.vector(vid).iter() {
                    collect_require_targets(heap, *v, out);
                }
            }
            Value::Map(mid) => {
                for (k, v) in heap.map_entries(mid).iter() {
                    collect_require_targets(heap, *k, out);
                    collect_require_targets(heap, *v, out);
                }
            }
            _ => {}
        }
    })
}

/// Every qualified symbol *name* (`"mod/name"`, module segment non-empty) appearing
/// anywhere in the un-expanded `forms` — the user-written references. Over-approximate
/// (includes quoted data and binder positions): it only ever WIDENS the file's world.
pub(super) fn collect_raw_qualified(heap: &Heap, forms: &[Value]) -> HashSet<String> {
    collect_all_syms(heap, forms)
        .into_iter()
        .filter_map(|s| {
            let n = value::symbol_name(s);
            match n.rfind('/') {
                Some(slash) if slash > 0 => Some(n),
                _ => None,
            }
        })
        .collect()
}

/// Populate the current file's import table from a `(defmodule … (:use …)/(:alias …))`
/// header WITHOUT evaling/compiling the header. In a whole-project check every module is
/// already loaded (`project--ensure-loaded`), so `check_file` can set up imports directly
/// instead of re-macroexpanding + re-evaling + re-compiling every file's header — the eval
/// path's per-file `provide`/`require`/`%refer` all-globals scans + `*module-docs*` rebind +
/// compile made a whole-project check O(files²).
///
/// Mirrors `defmodule`'s `(:use)`/`(:alias)` expansion (std/prelude.blsp) + the `%refer`/
/// `%alias` builtins exactly: `(:use mod)` refers mod's public (non-`--`) names bare;
/// `(:use mod :only [a b])` / `:refer` refers just those; `(:alias mod [:as short])` adds a
/// `short/` → `mod` prefix alias. A used module that isn't loaded (a bare-file check outside a
/// project) is `require`d first — rare, and correctness there beats the O(files²) speed win.
/// The modules a `(defmodule …)` header brings into scope, as the type-alias resolver
/// needs them (ADR-327): the ROOTED names of every `:use`d module —
/// a bare alias name resolves to one of theirs before the loaded-module-wide unique-suffix
/// rule — and each `(:alias mod :as short)` prefix, so `short/name` in a `sig` reaches
/// `mod/name`. Accumulated across a file's headers (the region model, ADR-223).
#[derive(Default)]
pub(crate) struct ImportScope {
    pub(crate) used: Vec<String>,
    pub(crate) aliases: Vec<(String, String)>,
    /// `(:use mod :only [name])` naming something `mod` does not define — the check the
    /// runtime `%refer` cannot make (KI-211: at refer time a module may be mid-load or
    /// image-backed), reported at the `(:use …)` clause.
    pub(crate) unknown_only: Vec<(Option<Pos>, String)>,
}

impl ImportScope {
    pub(super) fn extend(&mut self, other: ImportScope) {
        self.used.extend(other.used);
        self.aliases.extend(other.aliases);
        self.unknown_only.extend(other.unknown_only);
    }
}

pub(super) fn setup_check_imports(heap: &mut Heap, header: Value) -> ImportScope {
    let mut scope = ImportScope::default();
    // PASS A — parse the clauses into GC-STABLE data (`Symbol`s are Copy `u32`) with NO
    // eval, so nothing LOCAL is held across the `require` eval in pass B (which can collect
    // and relocate handles). Holding a parsed `Value` across that eval was a use-after-GC.
    enum Clause {
        // (module, Some(:only subset) | None = all public, the clause's position)
        Use(Symbol, Option<Vec<Symbol>>, Option<Pos>),
        UseExcept(Symbol, Vec<Symbol>), // (:use mod :exclude [names]) — all public minus names
        Alias(Symbol, Symbol),          // (short-prefix-name, module)
        UseInternals(Symbol),           // (:use-internals mod) — ADR-146 grant
        Shadow(Symbol),                 // (:shadow name …) — a declared root-macro shadow, ADR-423
    }
    let mut clauses: Vec<Clause> = Vec::new();
    {
        let Some(items) = list_items(heap, header) else {
            return scope;
        };
        // items = [defmodule, mod-name, doc?, clause...]; clauses follow name + optional doc.
        let first_clause = if matches!(items.get(2), Some(Value::Str(_))) {
            3
        } else {
            2
        };
        for clause in items.iter().skip(first_clause) {
            let Some(citems) = list_items(heap, *clause) else {
                continue;
            };
            let Some(Value::Keyword(kw_sym)) = citems.first() else {
                continue;
            };
            if value::symbol_is(*kw_sym, "use") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // `:only [a b]` → refer just those; `:exclude [a b]` → all public minus
                // those; no marker → all public. Mirrors the runtime `defmodule--use-clause`
                // vocabulary exactly (`:refer` is not a marker — the runtime rejects it, and
                // the checker's compile pass surfaces that as a "does not compile" diagnostic).
                let marker = citems.get(2);
                let is_only =
                    matches!(marker, Some(Value::Keyword(m)) if value::symbol_is(*m, "only"));
                let is_exclude =
                    matches!(marker, Some(Value::Keyword(m)) if value::symbol_is(*m, "exclude"));
                if is_only || is_exclude {
                    // `:only`/`:exclude [a b]` uses a VECTOR literal (also accepts a list), so
                    // read it with `seq_items` (vector + list) — NOT `list_items` (cons-only),
                    // which would miss a vector. Read-only, no alloc → no GC, so holding
                    // `items`/`citems` across it is safe.
                    let names: Vec<Symbol> = citems
                        .get(3)
                        .and_then(|sub| heap.seq_items(*sub).ok())
                        .map(|ns| {
                            ns.iter()
                                .filter_map(|n| match n {
                                    Value::Sym(s) => Some(*s),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if is_only {
                        let pos = heap.form_pos_only(*clause);
                        clauses.push(Clause::Use(mod_sym, Some(names), pos));
                    } else {
                        clauses.push(Clause::UseExcept(mod_sym, names));
                    }
                } else {
                    clauses.push(Clause::Use(mod_sym, None, None));
                }
            } else if value::symbol_is(*kw_sym, "use-internals") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // A grant is also a use, plus the ADR-146 internals key the privacy walk
                // consults. `(:use-internals mod :only [a b])` refers just those names, which
                // may be privates — so the grant goes FIRST, as the runtime emits it before
                // the refer: the `:only` arm of `Clause::Use` skips a private it sees no grant
                // for. Without `:only`, the module's publics refer bare, as `(:use mod)`.
                let is_only = matches!(citems.get(2),
                    Some(Value::Keyword(m)) if value::symbol_is(*m, "only"));
                clauses.push(Clause::UseInternals(mod_sym));
                if is_only {
                    // Read-only, no alloc → no GC (as in the `:use` arm above).
                    let names: Vec<Symbol> = citems
                        .get(3)
                        .and_then(|sub| heap.seq_items(*sub).ok())
                        .map(|ns| {
                            ns.iter()
                                .filter_map(|n| match n {
                                    Value::Sym(s) => Some(*s),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    clauses.push(Clause::Use(
                        mod_sym,
                        Some(names),
                        heap.form_pos_only(*clause),
                    ));
                } else {
                    clauses.push(Clause::Use(mod_sym, None, None));
                }
            } else if value::symbol_is(*kw_sym, "shadow") {
                for name in citems.iter().skip(1) {
                    if let Value::Sym(name) = *name {
                        clauses.push(Clause::Shadow(name));
                    }
                }
            } else if value::symbol_is(*kw_sym, "alias") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // `(:alias mod [:as short])`; `short` defaults to mod's last `/`-segment.
                let short = match (citems.get(2), citems.get(3)) {
                    (Some(Value::Keyword(m)), Some(Value::Sym(s)))
                        if value::symbol_is(*m, "as") =>
                    {
                        *s
                    }
                    _ => {
                        let mn = value::symbol_name(mod_sym);
                        value::intern(mn.rsplit('/').next().unwrap_or(&mn))
                    }
                };
                clauses.push(Clause::Alias(short, mod_sym));
            }
        }
    }
    // PASS B — apply. Only `Symbol`s (Copy, GC-stable) are held, so the `require` eval below
    // (standalone-file path) can safely collect. `module_public_exports` re-reads globals
    // fresh; `add_import` takes `Symbol`s.
    // Load `mod_sym` unless it's already loaded — the standalone path; in a whole-project
    // check every module is loaded up front, so this is a no-op. Advisory: any load error
    // is swallowed (the checker never gates on a missing module).
    //
    // "Already loaded" is the FEATURE registry, not "some `mod/*` global exists". The
    // latter was the bug: a std module whose namespace is *also* a kernel-primitive
    // namespace — `file`, with its 18 `file/…` primitives — looked loaded before its
    // `.blsp` had ever been read, so `(:use file)` imported the primitives only and every
    // Brood-level name in it (`walk-files`, `read-lines`, `regular?`) was reported unbound
    // in a single-file `brood --check` (the whole-project path was unaffected: it loads
    // every module first). `*features*` is exactly what the runtime `require-one` consults,
    // so the checker's notion of loaded now matches the runtime's.
    //
    // Erring toward "not loaded" is free: `require-one` is idempotent, so a spurious call
    // costs a map lookup; erring toward "loaded" is what produced the false positives.
    for clause in clauses {
        match clause {
            Clause::Use(mod_sym, subset, pos) => {
                // Package-rooted namespaces (ADR-070): an intra-package `(:use b)` — in a
                // dependency, or in the root project under its `:name` — roots to `pkg/b`.
                // The runtime `%refer` target roots via `%root-module-name`; the checker
                // must root the same way here or it scans `b/*` for exports that actually
                // live under `pkg/b/*` and flags every imported name unbound. External /
                // std / already-qualified names are left unchanged.
                let mod_sym = heap.root_module_name(mod_sym);
                let mod_name = value::symbol_name(mod_sym);
                scope.used.push(mod_name.clone());
                let prefix = format!("{}/", mod_name);
                ensure_loaded(heap, mod_sym);
                match subset {
                    Some(names) => {
                        // Mirror the runtime `%refer`: a module-private name in `:only`
                        // needs an internals grant, else the runtime refuses the load —
                        // so the checker must NOT treat it as a bound import either
                        // (otherwise it resolves-as-bound a name whose file won't load).
                        // `ensure_loaded` ran above, so the privacy record is exact.
                        let granted = heap
                            .import_of(crate::eval::macros::internals_grant_key(&mod_name))
                            .is_some();
                        // What the module defines, for naming an `:only` entry it does not
                        // (KI-211's follow-up). A module with no exports here is one the
                        // check could not load — say nothing about it rather than guess.
                        let exports: std::collections::HashSet<Symbol> =
                            deps::obs_module_exports(heap, &prefix)
                                .into_iter()
                                .map(|(bare, _)| bare)
                                .collect();
                        for bare in names {
                            let bare_name = value::symbol_name(bare);
                            let qual = value::intern(&format!("{}/{}", mod_name, bare_name));
                            if heap.is_private(qual) && !granted {
                                continue;
                            }
                            let defined = exports.contains(&bare)
                                || sigs::is_globally_bound(heap, qual)
                                || crate::eval::derive::image_sig_arity(heap, qual).is_some()
                                || protocol::ability_declared_by(heap, bare, &mod_name);
                            if !defined && !exports.is_empty() {
                                scope.unknown_only.push((
                                    pos,
                                    format!(
                                        "(:use {mod_name} :only [... {bare_name} ...]): \
                                         {mod_name} defines no public `{bare_name}`"
                                    ),
                                ));
                            }
                            heap.add_import_lazy(bare, qual);
                        }
                    }
                    None => {
                        for (bare, qual) in deps::obs_module_exports(heap, &prefix) {
                            heap.add_import_lazy(bare, qual);
                        }
                    }
                }
            }
            Clause::UseExcept(mod_sym, excluded) => {
                let mod_sym = heap.root_module_name(mod_sym); // ADR-070, as in Clause::Use
                let mod_name = value::symbol_name(mod_sym);
                scope.used.push(mod_name.clone());
                let prefix = format!("{}/", mod_name);
                ensure_loaded(heap, mod_sym);
                let excluded: std::collections::HashSet<Symbol> = excluded.into_iter().collect();
                for (bare, qual) in deps::obs_module_exports(heap, &prefix) {
                    if !excluded.contains(&bare) {
                        heap.add_import_lazy(bare, qual);
                    }
                }
            }
            Clause::UseInternals(mod_sym) => {
                let mod_sym = heap.root_module_name(mod_sym); // ADR-070, as in Clause::Use
                let key = crate::eval::macros::internals_grant_key(&value::symbol_name(mod_sym));
                heap.add_import(key, mod_sym);
            }
            Clause::Shadow(name) => {
                // What the runtime clause's `%declare-shadow` records, so checking the module
                // compiles it as loading does — without the shadow warning it declared away.
                let key = crate::eval::macros::shadow_declared_key(&value::symbol_name(name));
                heap.add_import(key, name);
            }
            Clause::Alias(short, mod_sym) => {
                // Root the alias TARGET (ADR-070) so `short/name` resolves to `pkg/mod/name`;
                // the local `short` prefix is unchanged.
                let mod_sym = heap.root_module_name(mod_sym);
                // The runtime clause `require-one`s the target; the checker must too, or the
                // module's `deftype`s are not in the alias table when a `sig` says `short/name`
                // (a CALL `short/name` infers its load lazily, ADR-227 — a type name cannot).
                ensure_loaded(heap, mod_sym);
                let key = value::intern(&format!("{}/", value::symbol_name(short)));
                heap.add_import(key, mod_sym);
                scope
                    .aliases
                    .push((value::symbol_name(short), value::symbol_name(mod_sym)));
            }
        }
    }
    scope
}
