//! Source positions, compilation context and definition sites — child of heap.
//!
//! What the reader and the loaders record ABOUT code rather than the code itself: the source
//! position of every LOCAL list form (`set_form_pos`/`form_pos`, the synthetic mark), the
//! per-process compilation context (current file, namespace, package context, the
//! forward-reference name set and the `(:use …)` import table), and the cross-file
//! definition sites + name facts (`def_site`, privacy, `NameMeta`) that goto-definition, the
//! checker and the LSP read (ADR-031, ADR-146). Split out of `heap.rs` on 2026-09-06 (handoff
//! item 5): a `use super::*` child, so it reaches `Heap`'s private fields exactly as before.

use super::*;

impl Heap {
    /// Record the source position of a LOCAL list form (no-op for atoms and
    /// forms in the shared regions). Called by the reader as it builds lists.
    pub fn set_form_pos(&mut self, v: Value, pos: crate::error::Pos) {
        if let Some(id) = v.as_pair() {
            if id.region() == crate::core::value::LOCAL {
                // Clone the pre-shared `Arc` (a refcount bump), never `Arc::from(&str)` —
                // that allocated and copied the path on every single form. See
                // `ColdHeap::current_file_arc`.
                let cold = self.cold_mut();
                let file = cold.current_file_arc.clone();
                cold.form_pos.insert(
                    form_pos_key(id),
                    FormPos {
                        pos,
                        file,
                        synthetic: false,
                    },
                );
            }
        }
    }

    /// Mark a LOCAL list form as expander-built (see [`FormPos`]). The form must already
    /// have a position — the expander stamps one first — so the mark rides that entry and
    /// `promote` carries both together.
    /// Set a LOCAL list form's position **with an explicit file**, bypassing
    /// `current_file_arc`. The expander's synthetic stamp needs this: it runs not only at
    /// load time but at LAZY expansion — an arm compiled at first call, a
    /// `%coverage-precompile`, a hot reload — where the file *currently being loaded* is a
    /// different file, or none. Stamping the ambient file there produced records whose
    /// position came from one file and whose file named another, which is how a nested
    /// `reflect/load`'s functions were coverage-attributed to the OUTER script
    /// (`std_attribution` + four `coverage_lines` tests red, 2026-08-29). The file must
    /// travel WITH the position it was derived from.
    pub fn set_form_pos_in_file(
        &mut self,
        v: Value,
        pos: crate::error::Pos,
        file: Option<Arc<str>>,
    ) {
        if let Some(id) = v.as_pair() {
            if id.region() == crate::core::value::LOCAL {
                self.cold_mut().form_pos.insert(
                    form_pos_key(id),
                    FormPos {
                        pos,
                        file,
                        synthetic: false,
                    },
                );
            }
        }
    }

    pub fn mark_synthetic(&mut self, v: Value) {
        if let Some(id) = v.as_pair() {
            if id.region() == crate::core::value::LOCAL {
                if let Some(e) = self.cold_mut().form_pos.get_mut(&form_pos_key(id)) {
                    e.synthetic = true;
                }
            }
        }
    }

    /// Copy `from`'s whole position record — position, file AND synthetic mark — onto the
    /// LOCAL list form `to`. What a REBUILD of a form must do (`macros::rebuild_list`): the
    /// rebuilt list is the same logical form, so it must answer "where are you" and "did the
    /// expander make you" exactly as the original did. Copying the position alone is how
    /// a namespace-rooted rebuild under `(defmodule …)` came to shed the synthetic mark, and
    /// the unused-`let` lint then warned on every destructured name in a module.
    pub fn copy_form_pos(&mut self, from: Value, to: Value) {
        let Some(from_id) = from.as_pair() else {
            return;
        };
        let entry = match from_id.region() {
            crate::core::value::LOCAL => self
                .cold()
                .and_then(|c| c.form_pos.get(&form_pos_key(from_id)).cloned()),
            crate::core::value::RUNTIME => self
                .runtime
                .position_of(from_id.index(), from_id.code_gen()),
            _ => None,
        };
        let (Some(entry), Some(to_id)) = (entry, to.as_pair()) else {
            return;
        };
        if to_id.region() == crate::core::value::LOCAL {
            self.cold_mut().form_pos.insert(form_pos_key(to_id), entry);
        }
    }

    /// Was this list form built by the expander rather than read from source? A form
    /// the user wrote — even one an expansion spliced in unchanged — answers false. Read
    /// from whichever table holds the form, so a promoted form answers the same as it
    /// did while LOCAL.
    pub fn is_synthetic(&self, v: Value) -> bool {
        let Some(id) = v.as_pair() else {
            return false;
        };
        match id.region() {
            crate::core::value::LOCAL => self
                .cold()
                .and_then(|c| c.form_pos.get(&form_pos_key(id)))
                .is_some_and(|e| e.synthetic),
            crate::core::value::RUNTIME => self
                .runtime
                .position_of(id.index(), id.code_gen())
                .is_some_and(|e| e.synthetic),
            _ => false,
        }
    }

    /// The recorded source position (and originating file, if known) of a list form.
    /// LOCAL pairs read the per-heap reader-stamped table; RUNTIME pairs read the
    /// shared table `promote` carried the position into (so `form-pos` works on a
    /// frozen `defn`/lambda body and a position survives a cross-node send). PRELUDE
    /// forms carry none.
    ///
    /// Use [`form_pos_only`](Self::form_pos_only) when only the line/col is needed.
    pub fn form_pos(&self, v: Value) -> Option<(crate::error::Pos, Option<Arc<str>>)> {
        if let Some(id) = v.as_pair() {
            match id.region() {
                crate::core::value::LOCAL => {
                    return self
                        .cold()
                        .and_then(|c| c.form_pos.get(&form_pos_key(id)))
                        .map(|e| (e.pos, e.file.clone()))
                }
                // Read with the handle's OWN generation: the two RUNTIME generations
                // share an index space, so an index-only lookup answers with whichever
                // generation last wrote that slot (see `RuntimeCode::positions`).
                crate::core::value::RUNTIME => {
                    return self
                        .runtime
                        .position_of(id.index(), id.code_gen())
                        .map(|e| (e.pos, e.file))
                }
                _ => {}
            }
        }
        None
    }

    /// Convenience: just the `Pos` part of [`form_pos`](Self::form_pos), for
    /// callers that don't need the file.
    pub fn form_pos_only(&self, v: Value) -> Option<crate::error::Pos> {
        self.form_pos(v).map(|(p, _)| p)
    }

    /// Set the file currently being loaded, returning the previous value so the
    /// caller can restore it (loads nest).
    pub fn set_current_file(&mut self, file: Option<String>) -> Option<String> {
        // Re-share the path once per `load`, not once per form — see `current_file_arc`.
        let shared: Option<Arc<str>> = file.as_deref().map(Arc::from);
        let cold = self.cold_mut();
        cold.current_file_arc = shared;
        std::mem::replace(&mut cold.current_file, file)
    }

    /// The file currently being loaded, exposed to Brood via `(current-file)`.
    pub fn current_file(&self) -> Option<&str> {
        self.cold().and_then(|c| c.current_file.as_deref())
    }

    // ----- current namespace (ADR-065) -----

    /// Set the namespace being compiled into (`None` = root), returning the prior
    /// value so the caller can restore it. File/module loaders save + reset to
    /// `None` per file; the `%in-ns` primitive sets it from an `(ns …)` form.
    pub fn set_compile_ns(&mut self, ns: Option<Symbol>) -> Option<Symbol> {
        std::mem::replace(&mut self.cold_mut().compile_ns, ns)
    }

    /// The namespace currently being compiled into, or `None` at root.
    pub fn compile_ns(&self) -> Option<Symbol> {
        self.cold().and_then(|c| c.compile_ns)
    }

    // ----- package-rooted namespaces (ADR-070) -----

    /// Enter a dependency's load: `prefix` is the dep's local name, `modules` the
    /// short module names it provides. While set, `root_module_name` roots an
    /// intra-package module reference to `prefix/name`. Returns the prior
    /// `(prefix, modules)` so the caller restores it (dep loads nest — a dep may
    /// `require` another dep). Passing `None` clears the context (root project / std).
    pub fn set_package_context(
        &mut self,
        prefix: Option<Symbol>,
        modules: HashSet<Symbol>,
    ) -> (Option<Symbol>, HashSet<Symbol>) {
        let cold = self.cold_mut();
        let prev_prefix = std::mem::replace(&mut cold.package_prefix, prefix);
        let prev_modules = std::mem::replace(&mut cold.package_modules, modules);
        // Both memos are properties of the context we just replaced — drop them so a
        // nested dep load doesn't inherit the outer package's answers: `rooted_ref_ic`
        // holds `mod/name` → rooted spellings, and `global_ic` may hold a value cached
        // under an unrooted key that resolved through one (see `global_lookup_cached`).
        // Context switches are load-time and rare, so clearing costs nothing at run time.
        self.rooted_ref_ic.borrow_mut().clear();
        self.global_ic.borrow_mut().clear();
        (prev_prefix, prev_modules)
    }

    /// The active package prefix (a dep's local name), or `None` outside a dep load.
    pub fn package_prefix(&self) -> Option<Symbol> {
        self.cold().and_then(|c| c.package_prefix)
    }

    /// The active package context as `(prefix, modules)` — the pair
    /// [`set_package_context`](Self::set_package_context) takes. Read to *propagate* the
    /// context to a spawned process: "which package is this code from?" is a property of
    /// the code, not of the process running it (see `spawn_impl`).
    pub fn package_context(&self) -> (Option<Symbol>, HashSet<Symbol>) {
        match self.cold() {
            Some(c) => (c.package_prefix, c.package_modules.clone()),
            None => (None, HashSet::new()),
        }
    }

    /// Root a referenced module name to the active package: if a dep load is active
    /// and `module` is one of that dep's provided modules, return `prefix/module`;
    /// otherwise return `module` unchanged. This is the one place the `foo/` prefix
    /// is applied — used by `%in-ns` (rooting a declared `(defmodule b)`) and the
    /// loader's `%root-module-name` (rooting `(:use b)`/`(:alias b …)`/`require`
    /// targets). An external name (std, another dep, already `foo/…`) is left alone.
    pub fn root_module_name(&mut self, module: Symbol) -> Symbol {
        let Some(prefix) = self.package_prefix() else {
            return module;
        };
        let is_intra = self
            .cold()
            .is_some_and(|c| c.package_modules.contains(&module));
        if !is_intra {
            return module;
        }
        let rooted = format!(
            "{}/{}",
            crate::core::value::symbol_name(prefix),
            crate::core::value::symbol_name(module)
        );
        crate::core::value::intern(&rooted)
    }

    /// Root an intra-package **qualified reference** — the counterpart of
    /// [`root_module_name`](Self::root_module_name) for a `mod/name` symbol appearing in
    /// *code*, rather than a module name in a `(:use …)`/`(:alias …)` clause. Inside
    /// project `bedit`, `commands/cmd-open` roots to `bedit/commands/cmd-open`.
    ///
    /// Without this the rooted model is asymmetric: `(:use commands)` roots its target, so
    /// the bare names import fine, but every explicit `commands/cmd-open` — and every
    /// `(eval 'commands/cmd-open)` behind a late-bound keymap — goes unbound. Rooting is
    /// meant to be *implied* (ADR-070), which has to include the qualified spelling.
    ///
    /// The module part is everything before the **last** `/` (a global's own name never
    /// contains one), so a nested module splits correctly: `editor/treesit/point-forward`
    /// asks about module `editor/treesit`, which no project provides, and is left bare.
    /// An already-rooted `bedit/commands/cmd-open` asks about `bedit/commands` — also not
    /// in the short-name set — so rooting is idempotent. `None` = nothing to root.
    pub(crate) fn root_qualified_ref(&self, sym: Symbol) -> Option<Symbol> {
        if let Some(&cached) = self.rooted_ref_ic.borrow().get(&sym) {
            return cached;
        }
        let answer = self.root_qualified_ref_uncached(sym);
        self.rooted_ref_ic.borrow_mut().insert(sym, answer);
        answer
    }

    /// [`root_qualified_ref`](Self::root_qualified_ref) without the memo — the rule itself.
    fn root_qualified_ref_uncached(&self, sym: Symbol) -> Option<Symbol> {
        let prefix = self.package_prefix()?;
        let name = crate::core::value::symbol_name_ref(sym);
        let split = name.rfind('/')?;
        let module = crate::core::value::intern(&name[..split]);
        if !self
            .cold()
            .is_some_and(|c| c.package_modules.contains(&module))
        {
            return None;
        }
        Some(crate::core::value::intern(&format!(
            "{}/{}",
            crate::core::value::symbol_name_ref(prefix),
            name
        )))
    }

    /// Record the bare names the current-namespace file will define, so the
    /// resolver can qualify forward references. Returns the prior set so the
    /// caller can restore it (loads nest).
    pub fn set_ns_known_names(&mut self, names: HashSet<Symbol>) -> HashSet<Symbol> {
        std::mem::replace(&mut self.cold_mut().ns_known_names, names)
    }

    /// Is `sym` (a bare name) known to be defined in the current namespace's file?
    pub fn ns_knows_name(&self, sym: Symbol) -> bool {
        self.cold().is_some_and(|c| c.ns_known_names.contains(&sym))
    }

    /// Install the per-module forward-ref pre-scan (ADR-223), returning the prior map so
    /// the caller can restore it (loads nest). Keyed by the **bare** module name each
    /// `(defmodule …)` declares — what `%in-ns` receives before rooting.
    pub fn set_ns_known_by_module(
        &mut self,
        map: HashMap<Symbol, HashSet<Symbol>>,
    ) -> HashMap<Symbol, HashSet<Symbol>> {
        std::mem::replace(&mut self.cold_mut().ns_known_by_module, map)
    }

    /// Make module `bare`'s region the active forward-ref set: each `%in-ns` switches
    /// `ns_known_names` to the module it opens, so a bare forward reference qualifies only
    /// against the CURRENT module's defs — the region model that lets several modules
    /// share one file (ADR-223). A no-op when no region is recorded for `bare` (the sticky
    /// REPL path, which has no whole-file pre-scan and relies on `ns_assume_own` instead),
    /// so it never clobbers that path's set.
    pub fn activate_ns_region(&mut self, bare: Symbol) {
        let names = self
            .cold()
            .and_then(|c| c.ns_known_by_module.get(&bare).cloned());
        if let Some(names) = names {
            self.cold_mut().ns_known_names = names;
        }
    }

    /// Compile the next form(s) as a namespace's **own** code with no whole-file
    /// pre-scan available — a runtime `eval`. A file loader scans every form's def
    /// head up front, so a forward reference inside a file has positive evidence
    /// (`ns_known_names`) and qualifies; `eval` sees one form at a time and cannot,
    /// so a reference to a name a *later* `eval` will define is left bare and then
    /// misses the module-qualified global (KI-24). With this set, the resolver's
    /// last resort flips: a bare name that is bound at root/prelude still falls
    /// through (so `+`/`map` keep working), but one bound *nowhere* is taken to be
    /// this namespace's, matching what the file pre-scan would have concluded.
    /// Returns the prior value so the caller can restore it (evals nest).
    pub fn set_ns_assume_own(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.cold_mut().ns_assume_own, on)
    }

    /// Should an otherwise-unresolvable bare name be taken as this namespace's own?
    pub fn ns_assume_own(&self) -> bool {
        self.cold().is_some_and(|c| c.ns_assume_own)
    }

    /// Record one more bare name as defined in the current namespace's file. Used
    /// by the resolver when it qualifies a `def` head whose name the up-front
    /// forward-ref scan missed — a name produced by a *macro* expansion (e.g.
    /// `defserver` → `(def counter …)`), which `scan_def_names` can't see in the
    /// raw form. Registering it before the def's body is resolved lets self-
    /// references (the recursion in `counter`'s loop) qualify to the same name.
    pub fn add_ns_known_name(&mut self, sym: Symbol) {
        self.cold_mut().ns_known_names.insert(sym);
    }

    /// Replace the current file's `(:use …)` import table, returning the prior one
    /// so the caller can restore it (loads nest). Maps bare → [`ImportEntry`].
    pub fn set_imports(
        &mut self,
        imports: HashMap<Symbol, ImportEntry>,
    ) -> HashMap<Symbol, ImportEntry> {
        std::mem::replace(&mut self.cold_mut().imports, imports)
    }

    /// Every `(:use …)` import in this process, `(bare, entry)`, sorted by bare name —
    /// the read half of [`set_imports`](Self::set_imports), for `%compile-context`.
    pub fn imports_snapshot(&self) -> Vec<(Symbol, ImportEntry)> {
        let mut out: Vec<(Symbol, ImportEntry)> = self
            .cold()
            .map(|c| c.imports.iter().map(|(k, v)| (*k, v.clone())).collect())
            .unwrap_or_default();
        out.sort_by_key(|(bare, _)| crate::core::value::symbol_name(*bare));
        out
    }

    /// Add one imported binding (bare name → qualified global). Used by `%refer`.
    /// The clash-handling in `%refer` (`refer_add`) calls the ambiguity helpers below
    /// instead when a second module contributes the same bare name.
    pub fn add_import(&mut self, bare: Symbol, qualified: Symbol) {
        self.cold_mut()
            .imports
            .insert(bare, ImportEntry::One(qualified));
    }

    /// Add an imported binding, demoting to ambiguous on a clash (ADR-235): a fresh name
    /// becomes `One`; a second module contributing the same name (or a further one on an
    /// already-`Ambiguous` entry) becomes `Ambiguous`; re-adding the identical qualified is
    /// a no-op. The lazy counterpart of `add_import`, sharing `%refer`'s clash semantics so
    /// the checker's import setup agrees with the runtime's. (No shadow warning — that is a
    /// runtime-only concern handled in `refer_add`.)
    pub fn add_import_lazy(&mut self, bare: Symbol, qualified: Symbol) {
        match self.import_of(bare) {
            Some(existing) if existing == qualified => {} // idempotent
            Some(_) => self.mark_import_ambiguous(bare, qualified),
            None if self.ambiguous_import_of(bare).is_some() => {
                self.mark_import_ambiguous(bare, qualified)
            }
            None => self.add_import(bare, qualified),
        }
    }

    /// The qualified global a bare name was `(:use …)`-imported to, if it resolves to a
    /// single module. `None` for an unimported name **and** for an ambiguous one — an
    /// ambiguous bare name has no single target, so callers that want a definite import
    /// (aliases, the resolver's happy path) correctly see nothing; use
    /// [`ambiguous_import_of`](Self::ambiguous_import_of) to detect the ambiguous case.
    pub fn import_of(&self, bare: Symbol) -> Option<Symbol> {
        match self.cold().and_then(|c| c.imports.get(&bare)) {
            Some(ImportEntry::One(q)) => Some(*q),
            _ => None,
        }
    }

    /// The candidate qualified names a bare name is `(:use …)`-imported to from **more
    /// than one** module — `Some(sorted candidates)` iff the name is ambiguous, else
    /// `None`. Used by the resolver to raise a use-site clash error (ADR-235).
    pub fn ambiguous_import_of(&self, bare: Symbol) -> Option<Vec<Symbol>> {
        match self.cold().and_then(|c| c.imports.get(&bare)) {
            Some(ImportEntry::Ambiguous(v)) => Some(v.clone()),
            _ => None,
        }
    }

    /// Record that a bare name is imported from two modules at once — demote a prior
    /// `One` to `Ambiguous`, or append a further candidate to an existing `Ambiguous`.
    /// Candidates are kept sorted and deduped so the use-site error reads deterministically.
    pub fn mark_import_ambiguous(&mut self, bare: Symbol, candidate: Symbol) {
        let entry = self
            .cold_mut()
            .imports
            .entry(bare)
            .or_insert(ImportEntry::Ambiguous(Vec::new()));
        let mut names = match entry {
            ImportEntry::One(q) => vec![*q],
            ImportEntry::Ambiguous(v) => std::mem::take(v),
        };
        if !names.contains(&candidate) {
            names.push(candidate);
        }
        names.sort_unstable(); // Symbol = u32; deterministic dedup order (the resolver sorts by name when it formats)
        *entry = ImportEntry::Ambiguous(names);
    }

    /// Every `(bare, qualified)` import pair in the current file's table — for the
    /// LSP to offer imported names as bare completion candidates (ADR-065 §6). Only the
    /// unambiguous (`One`) imports; an ambiguous name is not usable bare.
    pub fn imported_pairs(&self) -> Vec<(Symbol, Symbol)> {
        self.cold()
            .map(|c| {
                c.imports
                    .iter()
                    .filter_map(|(&b, e)| match e {
                        ImportEntry::One(q) => Some((b, *q)),
                        ImportEntry::Ambiguous(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // ===== Definition sites (cross-file xref; ADR-031) =========================

    /// If `form` is a top-level `def`/`defn`/`defmacro`, record its name's source
    /// location (the [`current_file`] + `pos`). Called by the file loaders on each
    /// *un-expanded* top-level form — before macroexpansion, so `defn`/`defmacro`
    /// (which lower to `def`) are still recognisable by their head and their span
    /// is intact. A no-op when no file is set (e.g. the REPL) or the form isn't a
    /// definition.
    ///
    /// A `(do …)` is descended into: a definer macro like `defrecord`/`defability`
    /// expands to a `do` wrapping several inner `def`/`defn`s (the constructor, its
    /// accessors, the ability's op dispatchers). The loaders call this on the
    /// *expanded* form too, so recording each inner def at the same call-site `pos`
    /// gives those macro-synthesized globals a def-site — otherwise cross-file
    /// goto-definition on a record constructor or ability op finds nothing (ADR-031).
    ///
    /// [`current_file`]: Self::current_file
    /// Like [`note_definition`], but also returns the names it recorded — the
    /// boot-cache writer's hook. The cache-hit boot no longer reads the raw prelude
    /// (that positioned read cost 3.5 ms of a 26 ms boot and produced nothing but
    /// these names), so the names the *un-expanded* form would have contributed are
    /// captured here, once, on the cold source boot and stored in the cache file.
    /// Returning them rather than re-deriving them on read keeps [`def_form_name`]
    /// the single definition of "what name does this form bind".
    ///
    /// [`note_definition`]: Self::note_definition
    pub fn note_definition_recording(
        &mut self,
        form: Value,
        pos: crate::error::Pos,
    ) -> Vec<Symbol> {
        let before = self.runtime.def_sites_read().len();
        let mut names = Vec::new();
        let Some(file) = self.cold().and_then(|c| c.current_file.clone()) else {
            return names;
        };
        self.collect_definitions(form, &file, pos, &mut names);
        debug_assert!(self.runtime.def_sites_read().len() >= before);
        names
    }

    /// [`note_definition_with_file`] with an out-parameter for the names recorded.
    ///
    /// [`note_definition_with_file`]: Self::note_definition_with_file
    fn collect_definitions(
        &mut self,
        form: Value,
        file: &str,
        pos: crate::error::Pos,
        names: &mut Vec<Symbol>,
    ) {
        if let Some(name) = self.def_form_name(form) {
            self.runtime.def_sites_write().insert(
                name,
                SourceLoc {
                    file: file.to_string(),
                    pos,
                },
            );
            names.push(name);
            return;
        }
        let ValueRef::Pair(p) = form.unpack() else {
            return;
        };
        let ValueRef::Sym(head) = self.car(p).unpack() else {
            return;
        };
        if !crate::core::value::symbol_is(head, kw::DO) {
            return;
        }
        let mut rest = self.cdr(p);
        while let ValueRef::Pair(cell) = rest.unpack() {
            self.collect_definitions(self.car(cell), file, pos, names);
            rest = self.cdr(cell);
        }
    }

    /// Record `name`'s definition site directly, without a form to read it from —
    /// the boot-cache reader's counterpart to [`note_definition_recording`]. Uses
    /// [`current_file`], so it is a no-op when no file is set, exactly like
    /// [`note_definition`].
    ///
    /// [`current_file`]: Self::current_file
    /// [`note_definition`]: Self::note_definition
    /// Every recorded def site, for the prelude image (ADR-314) to carry. The imaged boot
    /// evaluates no `def`, so nothing calls [`Heap::record_def_site`] there and stdlib
    /// `M-.` would go dark — the one user-visible thing ADR-138's text cache took care to
    /// keep working on its own fast path.
    pub fn def_sites_snapshot(&self) -> Vec<(Symbol, SourceLoc)> {
        self.runtime
            .def_sites_read()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }

    /// Reinstate a def site verbatim — the restore half of [`Heap::def_sites_snapshot`].
    /// Unlike `record_def_site` this takes the file explicitly rather than reading
    /// `current_file`, because the imaged boot is not "inside" a file when it runs.
    pub fn set_def_site(&self, name: Symbol, loc: SourceLoc) {
        self.runtime.def_sites_write().insert(name, loc);
    }

    pub fn record_def_site(&mut self, name: Symbol, pos: crate::error::Pos) {
        let Some(file) = self.cold().and_then(|c| c.current_file.clone()) else {
            return;
        };
        self.runtime
            .def_sites_write()
            .insert(name, SourceLoc { file, pos });
    }

    pub fn note_definition(&mut self, form: Value, pos: crate::error::Pos) {
        let Some(file) = self.cold().and_then(|c| c.current_file.clone()) else {
            return;
        };
        self.note_definition_with_file(form, &file, pos);
    }

    /// Record `form`'s def-site under `file`, descending into a `(do …)`. Split from
    /// [`note_definition`] so the `do` recursion resolves `current_file` just once.
    fn note_definition_with_file(&mut self, form: Value, file: &str, pos: crate::error::Pos) {
        if let Some(name) = self.def_form_name(form) {
            self.runtime.def_sites_write().insert(
                name,
                SourceLoc {
                    file: file.to_string(),
                    pos,
                },
            );
            return;
        }
        // Not a definer itself — if it's a `(do child…)`, record each child.
        let ValueRef::Pair(p) = form.unpack() else {
            return;
        };
        let ValueRef::Sym(head) = self.car(p).unpack() else {
            return;
        };
        if !crate::core::value::symbol_is(head, kw::DO) {
            return;
        }
        let mut rest = self.cdr(p);
        while let ValueRef::Pair(cell) = rest.unpack() {
            self.note_definition_with_file(self.car(cell), file, pos);
            rest = self.cdr(cell);
        }
    }

    /// The name a top-level `def`/`defn`/`defmacro` form binds, reading the head
    /// and first argument from the *un-expanded* form. `None` for anything else
    /// (including `(def (pattern) …)`, which has no plain name — deferred).
    fn def_form_name(&self, form: Value) -> Option<Symbol> {
        let ValueRef::Pair(p) = form.unpack() else {
            return None;
        };
        let ValueRef::Sym(head) = self.car(p).unpack() else {
            return None;
        };
        if !(crate::core::value::symbol_is(head, kw::DEF)
            || crate::core::value::symbol_is(head, kw::DEF_PRIVATE)
            || crate::core::value::symbol_is(head, kw::DEFN)
            || crate::core::value::symbol_is(head, kw::DEFN_PRIVATE)
            || crate::core::value::symbol_is(head, kw::DEFMACRO))
        {
            return None;
        }
        let ValueRef::Pair(rest) = self.cdr(p).unpack() else {
            return None;
        };
        match self.car(rest).unpack() {
            // Qualify the recorded name to the current namespace (ADR-065) so the
            // def-site key matches the global the resolver will actually define
            // (`foo/name`); a no-op at root or for an already-qualified name.
            ValueRef::Sym(name) => Some(match self.compile_ns() {
                Some(ns) => {
                    crate::eval::macros::qualify_name(&crate::core::value::symbol_name(ns), name)
                }
                None => name,
            }),
            _ => None,
        }
    }

    /// Where `name`'s global definition was loaded from, if recorded. Backs
    /// `(source-location 'name)`. The runtime table (user/project `def`s) takes
    /// precedence over the immutable prelude table, so redefining a prelude name
    /// reports the user's site, not the standard library's.
    pub fn def_site(&self, name: Symbol) -> Option<SourceLoc> {
        self.runtime
            .def_sites_read()
            .get(&name)
            .cloned()
            .or_else(|| self.prelude.def_sites.get(&name).cloned())
    }

    /// Is the global `sym` module-private (ADR-146)? The single predicate every
    /// semantic privacy check consults (see [`RuntimeCode::private`]). Since step 2
    /// moved the marker off the name onto the def form, a private is spelled exactly
    /// like a public, so there is no name-shaped fast-negative to take first: every
    /// query is the recorded-set lookup. That is O(1) regardless of how many privates
    /// exist, and the callers pre-filter (intra-module refs and granted modules never
    /// reach here), which is why dropping the old `--` fast path measured within noise.
    pub fn is_private(&self, sym: Symbol) -> bool {
        self.runtime.is_private_recorded(sym)
    }

    /// Record the qualified global `sym` as module-private (ADR-146). The public
    /// face of [`RuntimeCode::mark_private`], called by the `%mark-private`
    /// primitive that `defn-`/`def-` emit. Privacy is now a property the def form
    /// declares (recorded here), not one derived from the name.
    pub fn mark_private(&self, sym: Symbol) {
        self.runtime.mark_private(sym);
    }

    /// Record `sym`'s stability metadata (ADR-283) — the public face of
    /// [`RuntimeCode::set_meta`], called by the `%register-meta` primitive a `(meta …)`
    /// form emits.
    pub fn set_name_meta(&self, sym: Symbol, meta: NameMeta) {
        self.runtime.set_meta(sym, meta);
    }

    /// `sym`'s stability metadata, if a `(meta …)` recorded any.
    pub fn name_meta(&self, sym: Symbol) -> Option<NameMeta> {
        self.runtime.meta_of(sym)
    }

    /// A snapshot of this runtime's recorded module-private names. Used once, at
    /// prelude-build time, to capture the privates `%mark-private` recorded in the
    /// builder heap so they can seed each live runtime (the prelude is inserted, not
    /// re-evaluated — see [`RuntimeCode::seeded`]).
    /// Also the *snapshot* half of the `%isolate` bracket — see
    /// [`restore_private_names`](Self::restore_private_names).
    /// A snapshot of this runtime's recorded stability metadata (ADR-283) — the
    /// `(meta …)` facts, the sibling of [`private_names_snapshot`](Self::private_names_snapshot)
    /// and used at the same one place, for the same reason: the prelude is inserted into
    /// each live runtime, not re-evaluated, so a fact recorded by evaluating it in the
    /// builder heap has to be carried across explicitly or it is lost.
    pub fn name_meta_snapshot(&self) -> Vec<(Symbol, NameMeta)> {
        self.runtime
            .meta
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(s, m)| (*s, m.clone()))
            .collect()
    }

    pub fn private_names_snapshot(&self) -> Vec<Symbol> {
        self.runtime
            .private
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    /// **Replace** this runtime's recorded module-private set with `names` — the restore
    /// half of [`private_names_snapshot`](Self::private_names_snapshot).
    ///
    /// `%isolate` restores only the global *binding* table, so the Rust-side mark
    /// registries leaked across the boundary: a `defn-`/`def-` evaluated inside an
    /// isolate left its name marked private forever, even though the binding it
    /// described was rolled back. Bracketing the thunk with snapshot/restore fixes that.
    ///
    /// **Replaces, never unions** — that is the whole point. A mark added inside the
    /// isolate must be *dropped*, which a union would keep; and because the isolate also
    /// rolls back the bindings, a mark left behind describes a name that no longer
    /// exists. (`unmark_private` can only undo marks you can enumerate, which the caller
    /// cannot.) Takes the same `Vec<Symbol>` the snapshot produces, so the pair composes
    /// with no conversion.
    pub fn restore_private_names(&self, names: Vec<Symbol>) {
        self.runtime.restore_private(names);
    }
}

#[cfg(test)]
mod rt_position_tests {
    use super::*;
    use crate::error::Pos;

    /// The `(code_gen, index)` of a RUNTIME pair value — the identity
    /// [`RuntimeCode::positions`] is keyed by.
    fn rt_pair(v: Value) -> (usize, usize) {
        match v.unpack() {
            ValueRef::Pair(id) => {
                assert_eq!(id.region(), RUNTIME, "expected a promoted RUNTIME pair");
                (id.code_gen(), id.index())
            }
            _ => panic!("expected a pair"),
        }
    }

    /// Regression: the shared RUNTIME source-position table must be keyed by
    /// **`(code_gen, slab index)`**, not by the bare index.
    ///
    /// The two RUNTIME generations share one index space and a fresh generation starts
    /// its slabs at 0, so once aging is active a `def` into the new generation lands on
    /// the same index as a live form in the retained one. Keyed by index alone, the
    /// second write silently clobbered the first, and `(form-pos …)` — plus every error
    /// message and test-framework line lookup that goes through it — reported a
    /// stranger's position for still-live old-generation code.
    // The synthetic mark (ADR-297) rides the position record, so `promote` carries it: it
    // once lived in a side set beside the LOCAL table, and a promoted form silently lost
    // it — the checker then warned "unused let binding" on destructured names in every
    // loaded file while the LOCAL-heap unit test stayed green.
    #[test]
    fn the_synthetic_mark_survives_promotion() {
        let mut h = Heap::new();
        let pos = Pos { line: 3, col: 7 };
        let generated = h.alloc_pair(Value::int(1), Value::nil());
        h.set_form_pos(generated, pos);
        h.mark_synthetic(generated);
        let written = h.alloc_pair(Value::int(2), Value::nil());
        h.set_form_pos(written, pos);
        assert!(h.is_synthetic(generated));
        assert!(!h.is_synthetic(written));
        let (rg, rw) = (h.promote(generated), h.promote(written));
        assert_eq!(h.form_pos_only(rg), Some(pos), "the position still travels");
        assert!(h.is_synthetic(rg), "…and so does the mark");
        assert!(
            !h.is_synthetic(rw),
            "a form the user wrote stays not-synthetic"
        );
    }

    #[test]
    fn runtime_form_positions_are_keyed_by_generation() {
        let mut h = Heap::new();
        let pos_a = Pos { line: 11, col: 2 };
        let pos_b = Pos { line: 22, col: 4 };

        // A positioned form promoted into the current generation.
        let a = h.alloc_pair(Value::int(1), Value::nil());
        h.set_form_pos(a, pos_a);
        let ra = h.promote(a);
        assert_eq!(h.form_pos_only(ra), Some(pos_a));

        // Age: subsequent promotes land in the other generation, whose slab indices
        // restart at 0 and therefore collide with the retained generation's.
        assert!(h.age_runtime(), "the other generation slot should be empty");
        let b = h.alloc_pair(Value::int(2), Value::nil());
        h.set_form_pos(b, pos_b);
        let rb = h.promote(b);

        let ((gen_a, idx_a), (gen_b, idx_b)) = (rt_pair(ra), rt_pair(rb));
        assert_ne!(
            gen_a, gen_b,
            "the two forms must be in different generations"
        );
        assert_eq!(
            idx_a, idx_b,
            "the test only proves anything if the slab indices actually collide",
        );

        assert_eq!(h.form_pos_only(rb), Some(pos_b), "new-generation position");
        assert_eq!(
            h.form_pos_only(ra),
            Some(pos_a),
            "a promote into the fresh generation overwrote a LIVE old-generation form's \
             recorded position — the RUNTIME position table is keyed by bare slab index",
        );
    }
}
