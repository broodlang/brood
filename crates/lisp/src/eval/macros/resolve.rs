//! Namespace resolution (ADR-065) and the compile pass that drives it: qualifying a
//! macroexpanded form against the current namespace.

use super::*;

// ============================================================================
// Namespace resolution (ADR-065)
// ============================================================================
//
// Rewrite a *macroexpanded* top-level form against the current namespace
// (`heap.compile_ns`): qualify definition heads and free references to `ns/name`.
// Runs after `macroexpand_all`, before `eval`. At root (`compile_ns == None`) it
// is an identity no-op (one branch) — so the prelude and all non-namespaced code
// are untouched; only a file that opened `(ns …)` pays for the walk.
//
// Safety invariant: NEVER rewrite a binder/param/pattern position. Over-qualifying
// a local (treating a bound name as free) is a *silent* miscompile; under-qualifying
// a genuine reference is at worst a loud unbound error. So when the binder shape is
// uncertain (e.g. `%match` patterns, `&optional` defaults) we over-approximate the
// bound set and leave those positions verbatim — safe, occasionally incomplete.
// Data is inviolate: `quote`/`quasiquote` are skipped wholesale (a quoted symbol is
// a message tag / map key that travels by name across processes — ADR-034).

/// RAII scope for the per-file namespace-compilation state on the `Heap` — the four
/// fields that together decide how a bare reference resolves: `compile_ns`,
/// `ns_known_names`, `imports`, and `ns_assume_own`. A file loader (`load`,
/// `reload`, the file runner, `%load-string`) creates one before evaluating a file's
/// forms; **dropping it restores all four to the caller's values on every exit path —
/// normal return, an early `?`, or a panic unwinding through the loader** — so
/// namespace state can never leak across a file boundary and the four fields can never
/// fall out of sync (the hand-written save/restore this replaces bracketed only three,
/// leaking `ns_assume_own` out of `load`/`reload`/the file runner — a nested load then
/// wrongly inherited an interactive `eval`'s assume-own). It **owns** the `&mut Heap`
/// for its lifetime, so it is sound with no `unsafe`; reach the heap through
/// [`heap`](NsLoadScope::heap).
pub struct NsLoadScope<'a> {
    heap: &'a mut Heap,
    compile_ns: Option<Symbol>,
    known: HashSet<Symbol>,
    known_by_module: std::collections::HashMap<Symbol, HashSet<Symbol>>,
    imports: crate::core::heap::ImportTable,
    assume_own: bool,
    root_load_file: Option<Symbol>,
}

impl<'a> NsLoadScope<'a> {
    /// Save the caller's ns-state, then reset into a freshly-`load`ed file's scope: the
    /// ROOT namespace, an empty import table, this file's forward-reference pre-scan
    /// (only when the file opens a namespace — cheap otherwise), and `ns_assume_own`
    /// **off** (a loaded file carries a real pre-scan; a nested load must not inherit an
    /// interactive `eval`'s assume-own fallback). The saved state is restored on drop.
    pub fn enter(heap: &'a mut Heap, forms: &[Value]) -> Self {
        // The per-module forward-ref pre-scan (ADR-223). The *active* `ns_known_names`
        // starts empty: `compile_ns` is `None` until a `defmodule`'s `%in-ns` runs, and
        // resolution is a no-op at root, so no form is resolved before its region is
        // activated — `%in-ns` then switches the active set to the module it opens.
        let by_module = if file_opens_ns(heap, forms) {
            scan_regions(heap, forms)
        } else {
            std::collections::HashMap::new()
        };
        let compile_ns = heap.set_compile_ns(None);
        let known = heap.set_ns_known_names(HashSet::new());
        let known_by_module = heap.set_ns_known_by_module(by_module);
        let imports = heap.set_imports(crate::core::heap::ImportTable::default());
        let assume_own = heap.set_ns_assume_own(false);
        // A nested load is not the file around it: its registry writes credit its own
        // module, or itself (`credit_file`), never the namespace-less file that loaded it.
        let root_load_file = heap.set_root_load_file(None);
        Self {
            heap,
            compile_ns,
            known,
            known_by_module,
            imports,
            assume_own,
            root_load_file,
        }
    }

    /// The heap this scope owns for the duration of the load.
    #[inline]
    pub fn heap(&mut self) -> &mut Heap {
        self.heap
    }

    /// Credit the registry writes this file makes while no namespace is open to the file
    /// itself (ADR-582) — `reflect/load` of a file without a `defmodule`. Undone with the
    /// rest of the scope.
    pub fn credit_file(&mut self, file: Symbol) {
        self.heap.set_root_load_file(Some(file));
    }
}

impl Drop for NsLoadScope<'_> {
    fn drop(&mut self) {
        self.heap.set_root_load_file(self.root_load_file);
        self.heap.set_compile_ns(self.compile_ns);
        self.heap
            .set_ns_known_names(std::mem::take(&mut self.known));
        self.heap
            .set_ns_known_by_module(std::mem::take(&mut self.known_by_module));
        self.heap.set_imports(std::mem::take(&mut self.imports));
        self.heap.set_ns_assume_own(self.assume_own);
    }
}

/// The compile pass for one top-level form: expand macros, then resolve
/// namespaces. Every loader/driver runs forms through here before `eval` so the
/// runtime evaluator never sees an unexpanded macro or an unqualified namespaced
/// reference. At root (`compile_ns == None`) the resolve step is a no-op.
pub fn compile(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    // Module privacy is REAL (ADR-146): from inside a module, a hand-written
    // qualified reference to another module's `--` name is a compile error,
    // unless that module was granted with `(:use-internals mod)` (the
    // @testable seam). Enforced on the PRE-expansion source deliberately: a
    // module's macros may expand to its own private helpers inside any file
    // (the test framework's `describe`/`test` → `test/test--run` pattern) —
    // privacy governs what an author can *type*, and macro templates already
    // live behind `quasiquote`, which the walk skips. Top-level / REPL code
    // (no namespace) stays unrestricted — the live-hacking hatch hot reload
    // depends on.
    if let Some(ns) = heap.compile_ns() {
        enforce_private_refs(heap, form, &value::symbol_name(ns), None, 0)?;
    }
    let expanded = macroexpand_all(heap, form, env)?;
    // Inferred requires from qualified references (ADR-227 follow-up): arm recording so
    // `resolve` notes each qualified reference's module, then require them — before eval,
    // so `mod/name` is bound. At the root region (script/REPL) `resolve` is identity, so
    // additionally scan the form for qualified references there. (A qualified macro/call
    // head is already loaded eagerly during macroexpand.)
    let resolved = {
        let _recording = crate::eval::derive::RecordingScope::enter();
        let resolved = resolve(heap, expanded);
        if heap.compile_ns().is_none() {
            crate::eval::derive::scan_root_refs(heap, resolved);
        }
        resolved
    };
    // A bare name used in this form that is `(:use …)`-imported from two or more modules is
    // a use-site clash (ADR-235) — raise it now, at the point of use, naming the candidates.
    if let Some(error) = crate::eval::derive::take_ambiguous_error() {
        return Err(error);
    }
    // `drain_pending` loads any inferred modules, which collects — so `resolved`
    // (a LOCAL handle we still need for the quasiquote pass below) must be rooted
    // across it, or it goes stale (use-after-GC). The nested loads push and truncate
    // their own roots back to this base, so our slot stays valid throughout.
    let roots_base = heap.roots_len();
    let resolved_root = heap.root(resolved);
    if let Err(error) = crate::eval::derive::drain_pending(heap, env) {
        heap.truncate_roots(roots_base);
        return Err(error);
    }
    let resolved = heap.read_root(resolved_root);
    // Final step: expand auto-gensym-FREE `quasiquote`s into builder code so the VM
    // can compile arms that use them (a raw `quasiquote` special form otherwise
    // defers the WHOLE arm to the tree-walker — the dominant cost of macro-heavy
    // work, e.g. the advisory checker expanding every `defn`, ADR-119). Runs after
    // `resolve`, so a namespaced template's free refs are already qualified; produces
    // exactly what the runtime `expand_quasiquote` would, so behaviour is unchanged —
    // only the timing moves (once, at compile) for a template whose expansion is
    // deterministic. A `#`-autogensym template is LEFT for the runtime path (its
    // gensyms must be fresh per invocation, which a once-at-compile expansion freezes).
    let out = expand_static_quasiquotes(heap, resolved);
    heap.truncate_roots(roots_base);
    // The warning port is Brood code that may collect: keep `out` rooted across it.
    let out_root = heap.root(out);
    flush_pending_warnings(heap, env);
    let out = heap.read_root(out_root);
    heap.truncate_roots(roots_base);
    Ok(out)
}

/// The import-table key under which `(:use-internals mod)` records its grant —
/// the `%alias` trick: a leading `/` cannot arise from any real qualified
/// reference, so the key can never collide with a genuine import.
pub(crate) fn internals_grant_key(mod_name: &str) -> value::Symbol {
    value::intern(&format!("/internals/{mod_name}"))
}

/// The import-table key under which `(:shadow name)` records that this file shadows the root
/// macro `name` on purpose (ADR-423) — the same leading-`/` trick as [`internals_grant_key`].
pub(crate) fn shadow_declared_key(name: &str) -> value::Symbol {
    value::intern(&format!("/shadow/{name}"))
}

/// A module definition of a bare name the root binds to a MACRO: inside the module its own
/// definition now wins (ADR-423), so every bare `(name …)` there stops expanding the macro —
/// a change of meaning worth a word, unless the header declared it with `(:shadow name)`.
/// Shadowing a root FUNCTION stays silent: modules owning `get`/`map`/`count` is the
/// designed style (ADR-236). Once per definition per process; `BROOD_NO_SHADOW_WARN` mutes
/// it, as it mutes the import-shadow warning.
fn warn_macro_shadow(heap: &Heap, ns_name: &str, name: value::Symbol) {
    let bare = value::symbol_name_ref(name);
    if bare.contains('/')
        || bare.starts_with('%')
        || !matches!(
            heap.env_get(value::EnvId::GLOBAL, name).map(|v| v.unpack()),
            Some(ValueRef::Macro(_))
        )
        || heap.import_of(shadow_declared_key(bare)).is_some()
        || std::env::var_os("BROOD_NO_SHADOW_WARN").is_some()
    {
        return;
    }
    static WARNED: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);
    let key = format!("{ns_name}/{bare}");
    let first = WARNED
        .lock()
        .map(|mut w| w.get_or_insert_with(HashSet::new).insert(key.clone()))
        .unwrap_or(true);
    if first {
        // Queued, not printed: resolve runs with collection and macro expansion blocked, so it
        // cannot call the `*err*` port. `compile` writes the queue once the pass is done.
        PENDING_WARNINGS.with(|q| {
            q.borrow_mut().push(format!(
                "warning: `{key}` shadows the prelude macro `{bare}`: inside `{ns_name}` a bare \
                 `({bare} …)` now means this definition. Reach the macro as `/{bare}`; declare \
                 the shadow with (:shadow {bare}) in the defmodule header to silence this."
            ))
        });
    }
}

thread_local! {
    static PENDING_WARNINGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Write the warnings the resolve pass queued, through the `*err*` port.
fn flush_pending_warnings(heap: &mut Heap, env: EnvId) {
    let pending = PENDING_WARNINGS.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for message in pending {
        crate::builtins::warn_line(heap, env, &message);
    }
}

/// Enforce module privacy (ADR-146) over a resolved form: an error for any
/// **evaluated** qualified reference `m/name` where `name` is recorded
/// module-private in `m` (a `defn-`/`def-` definition) and `m` is neither the
/// current namespace nor a module granted via `(:use-internals m)`. `pos` tracks
/// the nearest enclosing form's position; `level` is the quasiquote nesting depth
/// (0 = an evaluated context). A symbol is a reference only at level 0 — inside a
/// `` `quasiquote `` template it is data, UNLESS an `~unquote` brings it back to
/// level 0 (that IS an evaluated reference, so `` `(~other/secret) `` is still
/// caught). `quote` is always data. A macro template referencing its OWN module's
/// private is fine (`m == cur_ns`), so this doesn't false-flag them.
///
/// Since a private name is now spelled identically to a public one (privacy is a
/// def-site fact, not a name marker), privacy is judged against the **record**
/// (`Heap::is_private`), which requires `m` to be loaded — a reference into a
/// module that was never loaded is not recorded, so it falls through to the normal
/// unbound-reference error at eval rather than a privacy message.
fn enforce_private_refs(
    heap: &Heap,
    form: Value,
    cur_ns: &str,
    pos: Option<crate::error::Pos>,
    level: u32,
) -> Result<(), LispError> {
    match form.unpack() {
        ValueRef::Sym(s) if level == 0 => {
            let name = value::symbol_name_ref(s);
            if let Some(slash) = name.rfind('/') {
                let (m, bare) = (&name[..slash], &name[slash + 1..]);
                // Resolve a module alias (`(:alias mod :as short)`) so the rule keys
                // on the REAL module, matching how the reference will resolve. A file with
                // no alias skips spelling the `m/` key: no lookup of it could hit.
                let alias_target = if heap.imports_may_have_aliases() {
                    heap.import_of(value::intern(&format!("{m}/")))
                } else {
                    None
                };
                // The real module, and — when it is already a symbol — the reference's
                // resolved spelling `module/bare`. Unaliased and unrooted, that is `s`
                // itself; rooted, it is the rooted symbol (`prefix/m/bare`, whose last
                // segment is `bare`); only an alias has to spell it, and does so below,
                // once the cheap exits have passed.
                let (module, known_resolved): (&str, Option<value::Symbol>) = match alias_target {
                    Some(target) => (value::symbol_name_ref(target), None),
                    // Not an alias: root it (ADR-070). `cur_ns` is the ROOTED namespace
                    // (`%in-ns` roots what `(defmodule tutor)` declares, so inside project
                    // `bedit` it is `bedit/tutor`), so an intra-project reference must root
                    // before the comparison — otherwise `tutor/tutor-line-face`, a module
                    // reaching its OWN helper, reads as a foreign private access. Alias
                    // resolution stays first, mirroring `resolve_reference`'s order; this
                    // walk sees the pre-rewrite form, hence the rooting here as well.
                    None => match heap.root_qualified_ref(s) {
                        Some(rooted) => {
                            let spelled = value::symbol_name_ref(rooted);
                            (
                                &spelled[..spelled.rfind('/').unwrap_or(spelled.len())],
                                Some(rooted),
                            )
                        }
                        None => (m, Some(s)),
                    },
                };
                let m = module;
                // A reference to the current module, or one this file was granted
                // internals for, is never a foreign-private access — exit before the
                // record lookup (also the cheap common case, no `is_private` call). A file
                // with no grant skips spelling the grant key.
                if m.is_empty()
                    || m == cur_ns
                    || (heap.imports_may_grant_internals()
                        && heap.import_of(internals_grant_key(m)).is_some())
                {
                    return Ok(());
                }
                // Foreign module: privacy is the recorded fact (ADR-146). The name is
                // spelled identically public-or-private, so consult the record for the
                // resolved `m/name`. An unloaded `m` has no record → not rejected here
                // (it becomes a normal unbound reference at eval).
                let resolved =
                    known_resolved.unwrap_or_else(|| value::intern(&format!("{m}/{bare}")));
                if heap.is_private(resolved) {
                    let mut e = LispError::runtime(format!(
                        "`{m}/{bare}` is module-private to `{m}` (ADR-146). Call it from \
                         `{m}`, make it public (`defn`/`def` rather than `defn-`/`def-`), \
                         or — for a test/tool that genuinely needs the internals — grant \
                         access with (:use-internals {m}) in this module's header."
                    ));
                    if let Some(p) = pos {
                        e = e.with_pos(p);
                    }
                    return Err(e);
                }
            }
            Ok(())
        }
        ValueRef::Sym(_) => Ok(()), // level > 0: template data, not a reference
        ValueRef::Pair(p) => {
            let (car, cdr) = heap.pair(p);
            // Quote/quasiquote/unquote adjust the evaluated-vs-data context.
            if let ValueRef::Sym(h) = car.unpack() {
                match value::symbol_name_ref(h) {
                    // `(quote X)` at the evaluated level — X is inert data, never
                    // a reference. But INSIDE a `` `quasiquote `` (level > 0) a
                    // `(quote …)` is just a 2-element list template: the reader's
                    // quasiquote still splices any `~unquote` nested within it
                    // (`` `(quote ~(m/priv--x)) `` evaluates the unquote), so it
                    // must NOT short-circuit there — fall through and keep walking
                    // at the same level so the nested unquote is still checked.
                    "quote" if level == 0 => return Ok(()),
                    // `` `(quasiquote X) `` — X is one level deeper (more data).
                    "quasiquote" => {
                        return enforce_private_refs(heap, cdr, cur_ns, pos, level + 1);
                    }
                    // `~X` / `~@X` — one level shallower; at level 1 this returns
                    // to the evaluated context, so X's refs ARE checked.
                    "unquote" | "unquote-splicing" => {
                        return enforce_private_refs(
                            heap,
                            cdr,
                            cur_ns,
                            pos,
                            level.saturating_sub(1),
                        );
                    }
                    _ => {}
                }
            }
            let here = heap.form_pos_only(form).or(pos);
            enforce_private_refs(heap, car, cur_ns, here, level)?;
            enforce_private_refs(heap, cdr, cur_ns, here, level)
        }
        ValueRef::Vector(id) => {
            let here = heap.form_pos_only(form).or(pos);
            for &item in heap.vector(id).to_vec().iter() {
                enforce_private_refs(heap, item, cur_ns, here, level)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Does `form` contain a `#`-suffixed (auto-gensym) symbol anywhere?
fn has_autogensym(heap: &Heap, form: Value) -> bool {
    has_autogensym_at(heap, form, 0)
}

/// The walk behind [`has_autogensym`]. Two things keep it off the native stack for a
/// template built at runtime (the reader caps source nesting, `reflect/eval` of a
/// constructed form does not): a list SPINE is walked in the loop, so a long flat
/// template costs no depth, and descent stops at `MAX_DEPTH` — a template that deep
/// cannot expand anyway (`qq_elem` refuses it), so answering `false` there only picks
/// which path reaches that error. Unbounded, a deep `(quasiquote (unquote …))` chain
/// would overflow the stack — an uncatchable abort.
fn has_autogensym_at(heap: &Heap, form: Value, depth: u32) -> bool {
    if depth >= MAX_DEPTH {
        return false;
    }
    let mut cur = form;
    loop {
        match cur.unpack() {
            ValueRef::Sym(s) => {
                let n = value::symbol_name_ref(s);
                return n.len() > 1 && n.ends_with('#');
            }
            ValueRef::Pair(p) => {
                let (car, cdr) = heap.pair(p);
                if has_autogensym_at(heap, car, depth + 1) {
                    return true;
                }
                cur = cdr;
            }
            ValueRef::Vector(id) => {
                return heap
                    .vector(id)
                    .to_vec()
                    .iter()
                    .any(|&it| has_autogensym_at(heap, it, depth + 1));
            }
            ValueRef::Map(id) => {
                return heap.map_entries(id).iter().any(|(k, v)| {
                    has_autogensym_at(heap, *k, depth + 1) || has_autogensym_at(heap, *v, depth + 1)
                });
            }
            // A set's elements are template positions too — skipping them meant an
            // `x#` inside `` `#{x#} `` never took the runtime (fresh-gensym) path.
            ValueRef::Set(id) => {
                return heap
                    .map_entries(id)
                    .iter()
                    .any(|(k, _)| has_autogensym_at(heap, *k, depth + 1));
            }
            _ => return false,
        }
    }
}

/// See [`compile`]: rewrite every auto-gensym-free `quasiquote` into builder code.
fn expand_static_quasiquotes(heap: &mut Heap, form: Value) -> Value {
    expand_qq_rec(heap, form, 0).0
}

/// Returns `(rewritten, changed)` — `changed` avoids rebuilding an unchanged list
/// (Value has no cheap identity compare), so a quasiquote-free tree is returned as-is.
fn expand_qq_rec(heap: &mut Heap, form: Value, depth: u32) -> (Value, bool) {
    // This rewrite is an optimisation (the evaluator expands a `quasiquote` it did not
    // reach, one level per tail-loop turn), so past the cap the form is simply left
    // alone. The expansion walk never descends into a template, so a runtime-built
    // `(quasiquote (unquote (quasiquote …)))` chain arrives here unbounded and would
    // overflow the native stack — an abort, not an error.
    if depth >= MAX_DEPTH {
        return (form, false);
    }
    // A VECTOR literal is an atom to `list_items`, but its elements are
    // unevaluated forms — so a quasiquote nested inside one (`[a b \`(do ~@xs)]`,
    // the shape `%receive-split` returns) must be walked too: a raw `quasiquote`
    // special form surviving into the arm defers the WHOLE arm to the tree-walker,
    // and everything it calls with it (§7.3).
    if let ValueRef::Vector(vid) = form.unpack() {
        let items = heap.vector(vid).to_vec();
        let mut out = Vec::with_capacity(items.len());
        let mut changed = false;
        for it in items {
            let (e, c) = expand_qq_rec(heap, it, depth + 1);
            changed |= c;
            out.push(e);
        }
        return if changed {
            (heap.alloc_vector(out), true)
        } else {
            (form, false)
        };
    }
    let items = match heap.list_items(form) {
        Some(i) => i,
        None => return (form, false), // atom / improper list — nothing to walk
    };
    if items.is_empty() {
        return (form, false);
    }
    if let ValueRef::Sym(h) = items[0].unpack() {
        if value::symbol_is(h, kw::QUOTE) {
            return (form, false); // pure data — never touch
        }
        if value::symbol_is(h, kw::QUASIQUOTE) {
            // `(quasiquote a b)` is malformed — the evaluator rejects it with an
            // arity error. Leave the form alone so it gets there: expanding only
            // `a` here would silently drop the tail (the same reasoning as the
            // compiler's `(quote a b)` deferral).
            if items.len() != 2 {
                return (form, false);
            }
            let template = items[1];
            if has_autogensym(heap, template) || template_introduces_binder(heap, template) {
                // Runtime-only: fresh gensyms per invocation — a `#` auto-gensym, or a
                // template whose own `let`/`fn` binder the hygiene pass renames (so two
                // nested expansions of one macro get distinct binders).
                return (form, false);
            }
            return match expand_quasiquote(heap, template) {
                // Recurse into the builder code so a quasiquote in an unquoted
                // sub-form is expanded too; on expander error keep the runtime form.
                // The builder is located at the backquote it replaces, so an error in
                // it — a bad `~@` splice — reports there, not at the enclosing call.
                Ok(builder) => {
                    let builder = expand_qq_rec(heap, builder, depth + 1).0;
                    if let Some((pos, file)) = heap.form_pos(form) {
                        super::stamp_synthetic(heap, builder, pos, file);
                    }
                    (builder, true)
                }
                Err(_) => (form, false),
            };
        }
    }
    let mut out = Vec::with_capacity(items.len());
    let mut changed = false;
    for it in items {
        let (e, c) = expand_qq_rec(heap, it, depth + 1);
        changed |= c;
        out.push(e);
    }
    if changed {
        // Runs after the expander's stamp, so a plain rebuild would leave every enclosing
        // form — up to the top-level `def` — without its position.
        (rebuild_list(heap, form, out), true)
    } else {
        (form, false)
    }
}

/// Does any top-level form open a namespace (head `ns`)? Cheap gate so the
/// forward-reference pre-scan only runs for namespaced files.
pub fn file_opens_ns(heap: &Heap, forms: &[Value]) -> bool {
    file_ns(heap, forms).is_some()
}

/// The namespace symbol a file declares via a top-level `(defmodule NAME …)`, or
/// `None`. One per file, so the first such form wins. Used by the advisory checker
/// to resolve qualified references without evaluating the header.
pub fn file_ns(heap: &Heap, forms: &[Value]) -> Option<Symbol> {
    forms.iter().find_map(|&f| defmodule_form_name(heap, f))
}

/// Every module a file declares, in source order (ADR-223: a file may open more than one
/// `(defmodule …)`). The bare names as written — `%in-ns` roots them. `file_ns` is the
/// first of these; the checker/LSP/indexer iterate the whole list.
pub fn file_modules(heap: &Heap, forms: &[Value]) -> Vec<Symbol> {
    forms
        .iter()
        .filter_map(|&f| defmodule_form_name(heap, f))
        .collect()
}

/// If `form` is a top-level `(defmodule NAME …)`, its `NAME`; else `None`.
pub(crate) fn defmodule_form_name(heap: &Heap, form: Value) -> Option<Symbol> {
    let items = heap.list_items(form)?;
    match (items.first()?.unpack(), items.get(1).map(|v| v.unpack())) {
        (ValueRef::Sym(h), Some(ValueRef::Sym(name))) if value::symbol_is(h, kw::DEFMODULE) => {
            Some(name)
        }
        _ => None,
    }
}

/// Pre-scan UNEXPANDED top-level forms for the bare names a file will define
/// (`def`/`defn`/`defmacro`/`defdyn` heads, recursively), so the resolver can
/// qualify a *forward* reference to a same-namespace name defined later in the
/// file. Skips `quote`/`quasiquote` data.
pub fn scan_def_names(heap: &Heap, forms: &[Value]) -> HashSet<Symbol> {
    scan_regions(heap, forms).into_values().flatten().collect()
}

/// Pre-scan UNEXPANDED top-level forms into **per-module regions** (ADR-223). Each
/// top-level `(defmodule M …)` opens a region that runs to the next `defmodule` or EOF;
/// the returned map gives, per module `M` (keyed by the **bare** name it was declared with
/// — what `%in-ns` receives), the bare def-heads defined in `M`'s region. This is the
/// region-aware core: with one module per file the sole entry equals the old whole-file
/// set (which is why [`scan_def_names`] is just its union). Names `def`ined BEFORE the
/// first `defmodule` are the **root region** — they `def` at root (`compile_ns` is still
/// `None` there) and belong to no module, so they never enter any region's set and a bare
/// reference to one correctly falls through to root instead of misqualifying to `M/name`.
pub fn scan_regions(heap: &Heap, forms: &[Value]) -> HashMap<Symbol, HashSet<Symbol>> {
    let mut by_module: HashMap<Symbol, HashSet<Symbol>> = HashMap::new();
    // `defdyn`-declared names are ambient (never namespaced), so they must not be a known
    // ns-local name in ANY region — even when a region also `def`s the knob. Collected
    // file-wide and subtracted at the end, so declaration order is irrelevant (a module
    // may read a knob near its top, declare it in the middle, and set it near the bottom).
    let mut ambient: HashSet<Symbol> = HashSet::new();
    let mut current: Option<Symbol> = None;
    for &form in forms {
        if let Some(m) = defmodule_form_name(heap, form) {
            current = Some(m);
            by_module.entry(m).or_default(); // an entry even if the region has no defs
            continue; // the `defmodule` form itself declares no module-level names
        }
        if let Some(m) = current {
            scan_def_form(heap, form, by_module.entry(m).or_default(), &mut ambient);
        }
        // else: a root-region form (before the first `defmodule`) — tracked by no module.
    }
    for names in by_module.values_mut() {
        for a in &ambient {
            names.remove(a);
        }
    }
    by_module
}

fn scan_def_form(
    heap: &Heap,
    form: Value,
    names: &mut HashSet<Symbol>,
    ambient: &mut HashSet<Symbol>,
) {
    let items = match heap.list_items(form) {
        Some(i) => i,
        None => return,
    };
    let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) else {
        return;
    };
    if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) {
        return;
    }
    let hn = value::symbol_name_ref(h);
    if (matches!(
        hn,
        kw::DEF | kw::DEF_PRIVATE | kw::DEFN | kw::DEFN_PRIVATE | kw::DEFMACRO | kw::DEFDYN
    ) || defines_by_convention(hn))
        && let Some(ValueRef::Sym(name)) = items.get(1).map(|v| v.unpack())
    {
        // An AMBIENT name is never namespaced, so it must not be pre-recorded as
        // a namespace-local name — that would qualify every reference in the file
        // to `ns/*name*` and lose the single root binding. Two ways to be
        // ambient: this form is the `defdyn` that declares it (the scan runs
        // before evaluation, so the runtime mark isn't set yet), or the name was
        // already declared dynamic elsewhere — e.g. `(def *load-path* …)` in a
        // module, rebinding the prelude's knob.
        if hn == kw::DEFDYN || heap.is_dynamic(name) {
            ambient.insert(name);
        } else if !value::symbol_name_ref(name).contains('/') {
            // Only bare names get pre-recorded; an already-qualified def head
            // needs no forward-ref help.
            names.insert(name);
        }
    }
    // Recurse so a def nested in a top-level `(do …)`/`(when …)` is still found.
    for &it in &items[1..] {
        scan_def_form(heap, it, names, ambient);
    }
}

/// Does a top-level form headed by `hn` **define** its second element as a name in the
/// current namespace? True for the literal `def…` special forms (handled by the caller)
/// and, by CONVENTION, for any other head spelled `def…` — which is every definition
/// macro in the tree: `defonce`, `defrecord`, `defmulti`, `defmethod`, `defability`,
/// `defbehaviour`, `defevent`, and third-party ones like hatch's `defhtml`/`deflive`.
///
/// The pre-scan reads UNEXPANDED forms, so it cannot ask a macro what it binds: at scan
/// time the file's own `(:use …)` has not run and the macro may not even be loaded yet.
/// Expanding here to find out would be both order-dependent and a second run of every
/// macro. The spelling is the one signal available before expansion, and it is a signal
/// the language already leans on everywhere else (`?` predicate, `-` private, earmuffs).
///
/// Without this, a name a macro defines is absent from the region's known-name set, so a
/// bare reference to it from a function written ABOVE the definition does not qualify to
/// `ns/name` — it falls through to root and is **unbound at runtime**, past a clean
/// `nest check`, a clean `--strict` and a green suite (brood KI-118). Below the
/// definition it happened to work, because by then `ns/name` is bound.
///
/// A false positive costs a bare reference to a ROOT binding of the same name being
/// qualified to `ns/name` inside this module — which is what a real definition would
/// have done anyway, and the head has to be spelled `def…` for it to arise.
fn defines_by_convention(hn: &str) -> bool {
    // `(defmodule M …)` names a MODULE, not a module-level binding; `scan_regions`
    // consumes it before this is reached, but the recursive descent can still see one.
    if hn == kw::DEFMODULE {
        return false;
    }
    // A qualified call — `(web/page/defhtml card …)` — is the same form as the bare one.
    hn.rsplit('/')
        .next()
        .is_some_and(|last| last.starts_with("def"))
}

/// Resolve `form` against `heap.compile_ns`. Identity when at root.
pub fn resolve(heap: &mut Heap, form: Value) -> Value {
    // Before anything strips a `/name` to a bare name: rename the locals that would
    // capture it, so the escape reaches the root binding under any local.
    let form = rename_escape_captures(heap, form);
    let ns = match heap.compile_ns() {
        Some(ns) => ns,
        // At ROOT there is no namespace to qualify against, but the `/name` root escape
        // (ADR-236) still has to mean something: a prelude macro emits `/get` so a module
        // defining `get` cannot capture it, and that same expansion must also work in a
        // root script and during the prelude's own boot — returned untouched, `/get` would
        // reach the evaluator literally, unbound.
        //
        // So root gets the escape rewrite and nothing else. It cannot use `resolve_sym`:
        // with an empty `ns_name` that function's qualifier would turn every free symbol
        // into `/name`, which is the bug inverted.
        //
        // A root `defmacro`'s templates get their call heads escaped first (ADR-423) — which
        // the strip leaves alone, since it skips templates.
        None => {
            let form = escape_macro_definitions(heap, form);
            return strip_root_escapes(heap, form);
        }
    };
    // Bounded compile walk — block the safepoint so the partially-built output tree
    // and the Rust-local Vecs aren't relocated/swept mid-walk (resolve allocates a
    // parallel tree, like `macroexpand_all`; it re-enters neither eval nor expand).
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    let ns_name = value::symbol_name_ref(ns);
    let resolved = resolve_walk(heap, form, ns_name, &[]);
    // After resolution an own-namespace head is `ns/name` and an import its qualified name,
    // so what a macro template still leaves bare and bound is a root name: pin it (ADR-423).
    escape_macro_definitions(heap, resolved)
}

/// Resolve a single **reference** symbol `s` against the heap's current namespace
/// context (`compile_ns` + `(:use …)` imports + `ns_known_names`), exactly as the
/// compile pass's reference resolution does. This is the shared entry point the
/// **LSP** uses (ADR-065 §4) so "what does this name mean here" can never disagree
/// with the runtime: bare `observe` in a `(:use observer)` file → `observer/observe`,
/// an own-namespace def → `ns/observe`, a prelude/root or unknown name → unchanged.
/// Identity at root (`compile_ns == None`). Read-only (no allocation).
pub fn resolve_reference(heap: &Heap, s: value::Symbol) -> value::Symbol {
    match heap.compile_ns() {
        Some(ns) => resolve_sym(heap, s, value::symbol_name_ref(ns), &[]),
        None => s,
    }
}

/// The global a lone reference symbol names, when the compile pass would resolve it to
/// exactly that binding and nothing else could intervene — the fast path that lets
/// `reflect/eval` of a late-bound name (a keymap command, a layer, a hook) be a table read
/// instead of a compile (ADR-465). `None` means "not provably simple": the caller runs the
/// full pass, which then raises or resolves exactly as before.
///
/// Simple means: the symbol resolves (namespace, imports, aliases, package rooting, the
/// `/name` root escape — the same [`resolve_sym`] the pass uses) to a bound global that is
/// not a macro; the bare name is not an ambiguous import (the pass raises a use-site clash);
/// and, inside a module, the reference is not a foreign private one (the pass raises a
/// privacy error). A dynamic var answers its innermost `binding`, as the compiled read does.
pub fn resolve_bound_global(heap: &Heap, s: value::Symbol, root: EnvId) -> Option<Value> {
    let resolved = match heap.compile_ns() {
        Some(ns) => {
            if heap.ambiguous_import_of(s).is_some() {
                return None;
            }
            let ns_name = value::symbol_name_ref(ns);
            enforce_private_refs(heap, Value::Sym(s), ns_name, None, 0).ok()?;
            resolve_sym(heap, s, ns_name, &[])
        }
        None => {
            let name = value::symbol_name_ref(s);
            if name.len() > 1 && name.starts_with('/') {
                value::intern_existing(&name[1..])?
            } else {
                s
            }
        }
    };
    match heap.env_get(root, resolved) {
        Some(found) if !matches!(found.unpack(), ValueRef::Macro(_)) => Some(found),
        _ => None,
    }
}

/// An **ambient** name — one that is never namespaced, so a `def` of it in any
/// module rebinds the single *root* binding (what makes `(def *load-path* …)` from
/// anywhere change the path the loader reads).
///
/// Ambient status comes from a **declaration**, not a spelling: the name must have
/// been declared with `defdyn`. A spelling rule would make an ordinary module-local
/// constant silently global — two modules that each wrote `(def *width* …)` would
/// share one binding. Earmuffs remain the *convention* for a knob (and the checker
/// reads them as one), but they do not change scoping: an
/// undeclared `(def *width* 10)` inside a module is `mod/*width*`, like every other
/// definition.
pub(crate) fn is_ambient(heap: &Heap, sym: value::Symbol) -> bool {
    heap.is_dynamic(sym)
}

/// Qualify a definition head: `bar` -> `ns/bar`; an already-`/`-qualified name, or
/// an ambient (`defdyn`-declared) name, is taken as-is. Shared with
/// `Heap::def_form_name` so def-site keys match.
pub fn qualify_name(heap: &Heap, ns_name: &str, name: value::Symbol) -> value::Symbol {
    let spelling = value::symbol_name_ref(name);
    if spelling.contains('/') || is_ambient(heap, name) {
        name
    } else {
        value::intern(&format!("{}/{}", ns_name, spelling))
    }
}

/// Is `s` a special form or core-macro keyword — syntax rather than a reference?
///
/// These are the names that are meaningful yet bound in **no** environment, so the
/// "bound nowhere ⇒ it must be ours" inference in [`resolve_sym`] would happily
/// rewrite `if` to `mod/if`. (Most of the list is prelude macros, which the root
/// check already covers; `if`/`fn`/`let`/… are the ones that genuinely need this.
/// Macro *calls* are gone by resolve time — `macroexpand_all` runs first — but a
/// special-form head survives expansion by definition.)
fn is_syntax_keyword(s: value::Symbol) -> bool {
    static SYNTAX: std::sync::LazyLock<crate::core::heap::SymbolMap<()>> =
        std::sync::LazyLock::new(|| {
            crate::builtins::SPECIAL_FORMS
                .iter()
                .map(|n| (value::intern(n), ()))
                .collect()
        });
    SYNTAX.contains_key(&s)
}

/// A symbol the quasiquote expander reads by NAME, which no namespace may own: the
/// `~`/`~@` markers and a `#`-suffixed auto-gensym. The file path never reaches them
/// (they are not known names), but the runtime `eval` path's assume-own rule qualifies
/// every unbound bare symbol, which would turn `(unquote x)` into `(mod/unquote x)` and
/// leave a `defmacro` through `eval` unable to use `~`.
fn is_template_marker(s: value::Symbol) -> bool {
    if value::symbol_is(s, kw::UNQUOTE) || value::symbol_is(s, kw::UNQUOTE_SPLICING) {
        return true;
    }
    let name = value::symbol_name_ref(s);
    name.len() > 1 && name.ends_with('#')
}

/// Resolve one free reference symbol. Qualify only with positive evidence the name
/// belongs to this namespace (already a `ns/name` global, or pre-scanned as a def
/// head this file will create); otherwise leave bare for root/prelude fall-through.
pub(super) fn resolve_sym(
    heap: &Heap,
    s: value::Symbol,
    ns_name: &str,
    locals: &[value::Symbol],
) -> value::Symbol {
    if locals.contains(&s) {
        return s;
    }
    let name = value::symbol_name_ref(s);
    if let Some(slash) = name.find('/') {
        // `/name` — the addressable ROOT/prelude namespace: an EMPTY module prefix
        // (leading `/`) resolves to the bare root binding, so a module that shadows a
        // prelude name (`(defn map …)` → `mod/map`) can still reach the original as
        // `/map`. A bare `/` — the division operator, empty
        // `rest` — is left alone. An empty prefix can never denote a real module (no
        // module has an empty name), so this collides with nothing.
        if slash == 0 && name.len() > 1 {
            return value::intern(&name[1..]);
        }
        // A qualified `prefix/rest`. If `prefix` is a module alias from `(:alias …)`
        // — stored in the import table under the slash-suffixed key `prefix/` so it
        // rides the same per-file lifecycle — rewrite to the real module path:
        // `conn/build` → `web/conn/build`. Otherwise it's already fully qualified. A file
        // that declared no alias skips spelling the key, which no lookup could find.
        let alias_target = if heap.imports_may_have_aliases() {
            heap.import_of(value::intern(&format!("{}/", &name[..slash])))
        } else {
            None
        };
        if let Some(target) = alias_target {
            return value::intern(&format!(
                "{}/{}",
                value::symbol_name_ref(target),
                &name[slash + 1..]
            ));
        }
        // ADR-070: an intra-package qualified reference roots to the active package, just
        // as `%in-ns` roots the `(defmodule …)` it declares and `%refer` roots a `(:use …)`
        // target — `commands/cmd-open` inside project `bedit` becomes
        // `bedit/commands/cmd-open`. Rooting is *implied*, so the qualified spelling has to
        // root too: otherwise the bare names import fine while every explicit `mod/name`
        // goes unbound, and the `--` privacy rule below compares a rooted `cur_ns` against
        // an unrooted `m` and rejects a module's reference to its OWN helper. Rooting here,
        // in the expand pass, bakes the rooted spelling into the compiled code — a std or
        // other-package name is left bare, and an already-rooted one is unchanged.
        if let Some(rooted) = heap.root_qualified_ref(s) {
            // An intra-package qualified reference infers its (rooted) module's require.
            crate::eval::derive::record_qualified(value::symbol_name_ref(rooted), ns_name);
            return rooted;
        }
        // A qualified reference `mod/name` infers `(require 'mod)` — no explicit require
        // needed (ADR-227 follow-up). Record it so `compile` loads the module before
        // eval (for a value in argument position; a call head is loaded eagerly during
        // macroexpand). Deferred because `resolve_sym` runs under GC/macro blocks.
        crate::eval::derive::record_qualified(name, ns_name);
        return s; // already qualified, no alias
    }
    if is_ambient(heap, s) {
        return s; // declared with `defdyn` — ambient/root, never namespaced
    }
    // Own namespace first (a same-named local def shadows an import), then a
    // `(:use …)` import, then root/prelude fall-through (left bare).
    if let Some(own) = own_definition(heap, s, ns_name, name) {
        own
    } else if let Some(candidates) = heap.ambiguous_import_of(s) {
        // Imported bare from two or more modules (ADR-235). A same-namespace def or a
        // local (checked above / at the top) would have shadowed it; reaching here means
        // the bare name is genuinely ambiguous. Record a use-site clash error — raised by
        // the driver after `resolve`, so it points at the form that used the name. The
        // returned symbol is irrelevant (compilation is about to be abandoned); leave it
        // bare so nothing spuriously resolves before the error surfaces.
        crate::eval::derive::record_ambiguous(s, candidates);
        s
    } else if let Some(imported) = heap.import_of(s) {
        imported
    } else if heap.ns_assume_own()
        && !is_syntax_keyword(s)
        && !is_template_marker(s)
        && heap.env_get(value::EnvId::GLOBAL, s).is_none()
    {
        // No pre-scan behind this form (a runtime `eval`), so "will this namespace
        // define it?" has no positive evidence to consult — see `set_ns_assume_own`.
        // Root/prelude still wins when it actually binds the name, so this only
        // catches a name bound NOWHERE: either this namespace's, defined by a later
        // `eval` (the case worth fixing), or a typo, which now reports the qualified
        // spelling. Deliberately after the import branch, so `(:use …)` still wins.
        qualified_in(ns_name, s, name)
    } else {
        s
    }
}

/// `ns/name` when this namespace defines the bare `s` (spelled `name`): the file's pre-scan
/// names it, or a global already binds `ns/name`.
///
/// The global half asks without interning: a free bare reference to a prelude name (`first`,
/// `map`) is the common case, and interning `mod/first` just to find it unbound grew the
/// never-freed symbol table by one dead name per (namespace, prelude name) pair. A spelling
/// that is not a symbol binds nothing — no global, staged load write or dynamic binding can
/// key on it — so absence of the symbol is absence of the binding. The one lookup that could
/// still hit is ADR-070's rooted fallback (`ns/name` → `prefix/ns/name`, inside a package
/// whose modules include `ns`); there the spelling is interned as before.
fn own_definition(
    heap: &Heap,
    s: value::Symbol,
    ns_name: &str,
    name: &str,
) -> Option<value::Symbol> {
    if heap.ns_knows_name(s) {
        return Some(qualified_in(ns_name, s, name));
    }
    let qualified = if heap.package_may_root(ns_name) {
        qualified_in(ns_name, s, name)
    } else {
        existing_qualified_in(ns_name, s, name)?
    };
    heap.env_get(value::EnvId::GLOBAL, qualified)
        .is_some()
        .then_some(qualified)
}

/// What the memo knows about the spelling `ns/bare`.
#[derive(Clone, Copy)]
enum QualifiedSpelling {
    /// It is this symbol — exact for good, since an interned symbol is never freed.
    Interned(value::Symbol),
    /// It was not a symbol when [`value::qualified_names_minted`] read this count; still
    /// true while the count is unchanged.
    Absent(u64),
}

/// The per-thread memo behind [`qualified_in`] and [`existing_qualified_in`].
#[derive(Default)]
struct QualifiedMemo {
    /// The namespace the entries are for.
    namespace: String,
    /// Per bare symbol, what its `namespace/bare` spelling is.
    by_bare: crate::core::heap::SymbolMap<QualifiedSpelling>,
    /// Reused to spell `namespace/bare` for a lookup, so a re-check allocates nothing.
    spelling: String,
}

thread_local! {
    /// `ns/bare` per bare symbol, for the namespace named last: every free bare reference
    /// and every macro head asks [`resolve_sym`] for its own-namespace spelling, and one file
    /// resolves under one namespace, so the spelling was formatted and re-interned per
    /// reference. A different namespace clears it.
    static QUALIFIED_IN_NS: std::cell::RefCell<QualifiedMemo> =
        std::cell::RefCell::new(QualifiedMemo::default());
}

/// Run `body` over the memo, cleared first if it holds another namespace's entries.
fn with_qualified_memo<T>(ns_name: &str, body: impl FnOnce(&mut QualifiedMemo) -> T) -> T {
    QUALIFIED_IN_NS.with(|memo| {
        let mut memo = memo.borrow_mut();
        if memo.namespace != ns_name {
            memo.namespace.clear();
            memo.namespace.push_str(ns_name);
            memo.by_bare.clear();
        }
        body(&mut memo)
    })
}

/// The symbol spelled `ns_name/name`, `name` being `bare`'s spelling — interned if it is not
/// one yet.
fn qualified_in(ns_name: &str, bare: value::Symbol, name: &str) -> value::Symbol {
    with_qualified_memo(ns_name, |memo| {
        if let Some(QualifiedSpelling::Interned(symbol)) = memo.by_bare.get(&bare) {
            return *symbol;
        }
        let symbol = value::intern(&format!("{ns_name}/{name}"));
        memo.by_bare
            .insert(bare, QualifiedSpelling::Interned(symbol));
        symbol
    })
}

/// The symbol spelled `ns_name/name` if one exists, never interning it. A remembered absence
/// is re-asked only after some qualified name has been minted since, so a namespace's
/// references to prelude names take the interner's lock once each, not once per reference.
fn existing_qualified_in(ns_name: &str, bare: value::Symbol, name: &str) -> Option<value::Symbol> {
    with_qualified_memo(ns_name, |memo| {
        match memo.by_bare.get(&bare) {
            Some(QualifiedSpelling::Interned(symbol)) => return Some(*symbol),
            Some(QualifiedSpelling::Absent(count)) if *count == value::qualified_names_minted() => {
                return None;
            }
            _ => {}
        }
        // Read the count BEFORE the lookup: a name minted after this read moves the count,
        // so the absence recorded below is re-asked rather than trusted.
        let count = value::qualified_names_minted();
        memo.spelling.clear();
        memo.spelling.push_str(ns_name);
        memo.spelling.push('/');
        memo.spelling.push_str(name);
        let found = value::intern_existing(&memo.spelling);
        let remembered = match found {
            Some(symbol) => QualifiedSpelling::Interned(symbol),
            None => QualifiedSpelling::Absent(count),
        };
        memo.by_bare.insert(bare, remembered);
        found
    })
}

/// `/name` → `name`, everywhere in `form`, and nothing else. The root-namespace half of
/// the escape (see [`resolve`]).
///
/// Two passes on purpose: `has_root_escape` is a read-only walk that allocates nothing, and
/// the rebuilding walk runs only when it finds one. Root is the *prelude's* namespace, so
/// this is on the boot path — and a `/name` is rare, so the overwhelmingly common answer is
/// "no rewrite needed, hand back the same tree".
pub(crate) fn strip_root_escapes(heap: &mut Heap, form: Value) -> Value {
    if !has_root_escape(heap, form) {
        return form;
    }
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    rewrite_root_escapes(heap, form)
}

/// Is `s` spelled `/name` — a leading slash with something after it? A bare `/` is the
/// division operator and is left alone.
pub(super) fn is_root_escape(s: value::Symbol) -> bool {
    let name = value::symbol_name_ref(s);
    name.len() > 1 && name.starts_with('/')
}

// Only CODE is rewritten — quasiquote `level` 0. A `quote` is data, and so is a template,
// the same rule the main resolver follows. That is load-bearing here rather than merely
// consistent: the prelude is itself root code, so a `defrecord`/`for` template containing
// `/get` passes through this pass at *definition* time. Stripping the escape there would hand
// the expansion a bare `get` and reinstate exactly the capture the escape exists to prevent —
// which it did, for one build. The escape must survive in the template and be stripped when
// the EXPANSION is resolved, at the call site. A `~` hole back to level 0 is code again (a
// macro's own computation, or a value spliced into a template built at run time) and is
// stripped like any other code.

fn has_root_escape(heap: &Heap, form: Value) -> bool {
    has_root_escape_at(heap, form, 0)
}

fn has_root_escape_at(heap: &Heap, form: Value, level: u32) -> bool {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form.unpack() {
        ValueRef::Sym(s) => level == 0 && is_root_escape(s),
        ValueRef::Pair(_) => match heap.list_items(form) {
            Some(items) => match nesting(&items, level) {
                Nesting::Data => false,
                Nesting::Enter(inner) => has_root_escape_at(heap, items[1], inner),
                Nesting::Same => items.iter().any(|&v| has_root_escape_at(heap, v, level)),
            },
            None => false,
        },
        ValueRef::Vector(_) | ValueRef::Map(_) | ValueRef::Set(_) => elements(heap, form)
            .into_iter()
            .any(|v| has_root_escape_at(heap, v, level)),
        _ => false,
    })
}

fn rewrite_root_escapes(heap: &mut Heap, form: Value) -> Value {
    rewrite_root_escapes_at(heap, form, 0)
}

fn rewrite_root_escapes_at(heap: &mut Heap, form: Value, level: u32) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form.unpack() {
        ValueRef::Sym(s) if level == 0 && is_root_escape(s) => {
            Value::symbol(value::intern(&value::symbol_name(s)[1..]))
        }
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(form) else {
                return form;
            };
            let out: Vec<Value> = match nesting(&items, level) {
                Nesting::Data => return form,
                Nesting::Enter(inner) => {
                    vec![items[0], rewrite_root_escapes_at(heap, items[1], inner)]
                }
                Nesting::Same => items
                    .into_iter()
                    .map(|v| rewrite_root_escapes_at(heap, v, level))
                    .collect(),
            };
            rebuild_seq_like(heap, form, out)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out = items
                .into_iter()
                .map(|v| rewrite_root_escapes_at(heap, v, level))
                .collect();
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (key, val) in entries {
                let key = rewrite_root_escapes_at(heap, key, level);
                let val = rewrite_root_escapes_at(heap, val, level);
                pairs.push((key, val));
            }
            heap.map_from_pairs(pairs)
        }
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let out = items
                .into_iter()
                .map(|v| rewrite_root_escapes_at(heap, v, level))
                .collect();
            heap.set_from_elems(out)
        }
        _ => form,
    })
}

thread_local! {
    static TEMPLATE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Quoted data inside a template: its symbols are left as written, and every `~` hole or
/// nested template in it is resolved as `resolve_list` resolves one.
fn resolve_quoted_holes(
    heap: &mut Heap,
    form: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form.unpack() {
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(form) else {
                return form;
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack())
                && (value::symbol_is(h, kw::UNQUOTE)
                    || value::symbol_is(h, kw::UNQUOTE_SPLICING)
                    || value::symbol_is(h, kw::QUASIQUOTE))
            {
                return resolve_list(heap, form, ns_name, locals);
            }
            let out: Vec<Value> = items
                .iter()
                .map(|&it| resolve_quoted_holes(heap, it, ns_name, locals))
                .collect();
            reuse_or_rebuild(heap, form, &items, out)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out: Vec<Value> = items
                .iter()
                .map(|&it| resolve_quoted_holes(heap, it, ns_name, locals))
                .collect();
            if all_unchanged(&items, &out) {
                return form;
            }
            heap.alloc_vector(out)
        }
        _ => form,
    })
}

/// The quasiquote level of the resolve walk: 0 in code, one more inside each template, one
/// less inside each `~` hole. The guard restores the previous level on drop.
struct TemplateDepth(u32);

impl TemplateDepth {
    fn enter() -> Self {
        Self::set(TEMPLATE_DEPTH.with(|d| d.get()) + 1)
    }
    /// A `~`/`~@` hole: one quasiquote level down (ADR-503). Out of the outermost template
    /// it is macro-time code again, where `/name` resolves as anywhere else; inside a
    /// nested one it is still template — the outer template's own.
    fn hole() -> Self {
        Self::set(TEMPLATE_DEPTH.with(|d| d.get()).saturating_sub(1))
    }
    fn set(n: u32) -> Self {
        TemplateDepth(TEMPLATE_DEPTH.with(|d| d.replace(n)))
    }
    fn inside() -> bool {
        TEMPLATE_DEPTH.with(|d| d.get() > 0)
    }
}

impl Drop for TemplateDepth {
    fn drop(&mut self) {
        TEMPLATE_DEPTH.with(|d| d.set(self.0));
    }
}

fn resolve_walk(heap: &mut Heap, form: Value, ns_name: &str, locals: &[value::Symbol]) -> Value {
    // Same deep-form stack safety as macroexpand_all_depth above.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        resolve_walk_inner(heap, form, ns_name, locals)
    })
}

fn resolve_walk_inner(
    heap: &mut Heap,
    form: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    match form.unpack() {
        // Inside a template a `/name` is the root escape the EXPANSION needs, resolved at the
        // call site — stripping it here, at definition, would hand every caller a bare name
        // to capture (ADR-423; the root half is `rewrite_root_escapes_at`'s levels).
        ValueRef::Sym(s) if is_root_escape(s) && TemplateDepth::inside() => form,
        ValueRef::Sym(s) => Value::symbol(resolve_sym(heap, s, ns_name, locals)),
        ValueRef::Pair(_) => resolve_list(heap, form, ns_name, locals),
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for it in items.iter().copied() {
                out.push(resolve_walk(heap, it, ns_name, locals));
            }
            if all_unchanged(&items, &out) {
                return form;
            }
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for &(k, v) in &entries {
                let k = resolve_walk(heap, k, ns_name, locals);
                let v = resolve_walk(heap, v, ns_name, locals);
                pairs.push((k, v));
            }
            let unchanged = entries.iter().zip(&pairs).all(
                |(&(key_before, value_before), &(key_after, value_after))| {
                    same_value(key_before, key_after) && same_value(value_before, value_after)
                },
            );
            if unchanged {
                return form;
            }
            heap.map_from_pairs(pairs)
        }
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let mut out = Vec::with_capacity(items.len());
            for e in items.iter().copied() {
                out.push(resolve_walk(heap, e, ns_name, locals));
            }
            if all_unchanged(&items, &out) {
                return form;
            }
            heap.set_from_elems(out)
        }
        other => other,
    }
}

fn resolve_list(heap: &mut Heap, form: Value, ns_name: &str, locals: &[value::Symbol]) -> Value {
    let items = match heap.list_items(form) {
        Some(i) => i,
        None => return form, // improper list — leave verbatim
    };
    if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
        if value::symbol_is(h, kw::QUOTE) {
            if TemplateDepth::inside() {
                // In a template a quote is data too, but its `~` holes still are holes:
                // `'~(helper x)` calls `helper`, and `~'~tag` reads `tag` (ADR-503).
                return resolve_quoted_holes(heap, form, ns_name, locals);
            }
            return form; // pure data — never descend (ADR-034)
        }
        if value::symbol_is(h, kw::QUASIQUOTE) {
            let _template = TemplateDepth::enter();
            // α (ADR-065 §7): descend the template so a macro's *free* references
            // qualify to the **defining** namespace — frozen here at macro-def time,
            // so the expansion resolves in any consumer. `~unquote`/`~@` contents
            // resolve as code (the macro's params are in `locals`); a nested `quote`
            // stays data; `#` auto-gensyms and template-local binders stay bare
            // (not known ns names). At root (`compile_ns == None`) the whole resolver
            // is a no-op, so prelude macro templates are untouched.
            let mut out = Vec::with_capacity(items.len());
            out.push(items[0]); // the `quasiquote` head itself
            for &it in &items[1..] {
                out.push(resolve_walk(heap, it, ns_name, locals));
            }
            return reuse_or_rebuild(heap, form, &items, out);
        }
        if (value::symbol_is(h, kw::UNQUOTE) || value::symbol_is(h, kw::UNQUOTE_SPLICING))
            && TemplateDepth::inside()
        {
            let _hole = TemplateDepth::hole();
            let mut out = Vec::with_capacity(items.len());
            for it in items.iter().copied() {
                out.push(resolve_walk(heap, it, ns_name, locals));
            }
            return reuse_or_rebuild(heap, form, &items, out);
        }
        if value::symbol_is(h, kw::DEF) || value::symbol_is(h, kw::DEFMACRO) {
            return resolve_def(heap, form, &items, ns_name, locals);
        }
        if value::symbol_is(h, kw::FN) {
            return resolve_fn(heap, form, &items, ns_name, locals);
        }
        if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
            return resolve_let(heap, form, &items, ns_name, locals);
        }
        // `/%match` is the form the pattern lowerings emit (ADR-638, ADR-674)
        if value::symbol_is(h, kw::MATCH_ENGINE) || value::symbol_is(h, "/%match") {
            return resolve_match(heap, form, &items, ns_name, locals);
        }
    }
    // A standard binding macro in a template has not expanded yet: its binders are local
    // over the whole form (an over-approximation, as `resolve_match` makes). Qualified, they
    // would escape the hygiene rename, which never renames a qualified name.
    if TemplateDepth::inside()
        && let Some(binders) = hygiene::shaped_binders(heap, &items)
    {
        let mut inner = locals.to_vec();
        for binder in binders {
            match binder {
                hygiene::ShapedBinder::Pattern(pattern) => {
                    collect_binder_syms(heap, pattern, &mut inner)
                }
                hygiene::ShapedBinder::Params(params) => {
                    collect_param_syms(heap, params, &mut inner)
                }
            }
        }
        let mut out = Vec::with_capacity(items.len());
        for it in items.iter().copied() {
            out.push(resolve_walk(heap, it, ns_name, &inner));
        }
        return reuse_or_rebuild(heap, form, &items, out);
    }
    // Generic: resolve every element (the head too — a call head resolves).
    let mut out = Vec::with_capacity(items.len());
    for it in items.iter().copied() {
        out.push(resolve_walk(heap, it, ns_name, locals));
    }
    reuse_or_rebuild(heap, form, &items, out)
}

/// `(def NAME value)` / `(defmacro NAME params body…)` — qualify NAME; resolve the
/// value (def) or the body with params bound (defmacro). Params left verbatim.
fn resolve_def(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let is_defmacro = matches!(items.first().map(|v| v.unpack()), Some(ValueRef::Sym(h)) if value::symbol_is(h, kw::DEFMACRO));
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // def / defmacro head, verbatim
    match items.get(1).map(|v| v.unpack()) {
        Some(ValueRef::Sym(name)) => {
            // Register the (bare) name as known before resolving the value, so a
            // self-reference in the body — e.g. the recursion in a `defserver`-
            // generated receive loop — qualifies to the same `ns/name` the head
            // gets. `scan_def_names` misses macro-defined names (it scans the raw,
            // unexpanded form), so without this a `(defn counter … (counter …))`
            // that came from a macro would bind `ns/counter` but recurse on bare
            // `counter` → unbound. Harmless for an already-qualified name.
            // …but never for an ambient (`defdyn`-declared) name: it stays bare, so
            // registering it would qualify the rest of the file's references to a
            // namespace-local name that is never defined.
            if !value::symbol_name_ref(name).contains('/') && !is_ambient(heap, name) {
                warn_macro_shadow(heap, ns_name, name);
                heap.add_ns_known_name(name);
            }
            out.push(Value::symbol(qualify_name(heap, ns_name, name)));
        }
        Some(other) => out.push(other), // not a symbol — leave (eval will complain)
        None => return reuse_or_rebuild(heap, form, items, out),
    }
    if is_defmacro {
        let params = items.get(2).copied().unwrap_or(Value::nil());
        out.push(params); // params verbatim
        let mut inner = locals.to_vec();
        collect_param_syms(heap, params, &mut inner);
        for &b in items.get(3..).unwrap_or(&[]) {
            out.push(resolve_walk(heap, b, ns_name, &inner));
        }
    } else {
        for &v in items.get(2..).unwrap_or(&[]) {
            out.push(resolve_walk(heap, v, ns_name, locals));
        }
    }
    reuse_or_rebuild(heap, form, items, out)
}

/// `(fn …)` — single-arity `(params body…)` or multi-arity
/// `(doc? (params body…)…)`. Params bind in their body; param lists left verbatim.
fn resolve_fn(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let parts = &items[1..];
    let (has_doc, clause_start) = match parts.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if parts.len() > 1 => (true, 1),
        _ => (false, 0),
    };
    let clauses = &parts[clause_start..];
    let multi = !clauses.is_empty() && clauses.iter().all(|&f| is_arity_clause(heap, f));
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // fn / lambda head
    if multi {
        if has_doc {
            out.push(parts[0]);
        }
        for &clause in clauses {
            out.push(resolve_arity_clause(heap, clause, ns_name, locals));
        }
    } else {
        let params = parts.first().copied().unwrap_or(Value::nil());
        // Binders verbatim, `&optional` defaults resolved.
        out.push(resolve_param_defaults(heap, params, ns_name, locals));
        let mut inner = locals.to_vec();
        collect_param_syms(heap, params, &mut inner);
        for &b in parts.get(1..).unwrap_or(&[]) {
            out.push(resolve_walk(heap, b, ns_name, &inner));
        }
    }
    reuse_or_rebuild(heap, form, items, out)
}

/// One `(params body…)` arity clause: params bind in the body.
fn resolve_arity_clause(
    heap: &mut Heap,
    clause: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let cparts = match heap.list_items(clause) {
        Some(c) if !c.is_empty() => c,
        _ => return clause,
    };
    let mut inner = locals.to_vec();
    collect_param_syms(heap, cparts[0], &mut inner);
    let mut out = Vec::with_capacity(cparts.len());
    out.push(resolve_param_defaults(heap, cparts[0], ns_name, locals));
    for &b in &cparts[1..] {
        out.push(resolve_walk(heap, b, ns_name, &inner));
    }
    reuse_or_rebuild(heap, clause, &cparts, out)
}

/// `(let/let*/letrec (s1 v1 …) body…)` — simple symbol binders post-expand
/// (patterns lowered to `%match`). Binders left verbatim; RHSs and body resolved
/// with binders in scope (sequential — a safe over-approximation for plain `let`).
fn resolve_let(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let letrec = matches!(items.first().map(|v| v.unpack()), Some(ValueRef::Sym(h)) if value::symbol_is(h, kw::LETREC));
    let binds = match items.get(1).and_then(|&b| heap.list_or_vector_items(b)) {
        Some(b) if b.len() % 2 == 0 => b,
        _ => return generic_resolve(heap, form, items, ns_name, locals),
    };
    let mut scope = locals.to_vec();
    if letrec {
        for &t in binds.iter().step_by(2) {
            collect_binder_syms(heap, t, &mut scope);
        }
    }
    let mut new_binds = Vec::with_capacity(binds.len());
    let mut i = 0;
    while i < binds.len() {
        let target = binds[i];
        let rhs_r = resolve_walk(heap, binds[i + 1], ns_name, &scope);
        new_binds.push(target); // binder verbatim
        new_binds.push(rhs_r);
        if !letrec {
            collect_binder_syms(heap, target, &mut scope);
        }
        i += 2;
    }
    let new_bind_form = if all_unchanged(&binds, &new_binds) {
        items[1]
    } else {
        rebuild_seq_like(heap, items[1], new_binds)
    };
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]);
    out.push(new_bind_form);
    for &b in items.get(2..).unwrap_or(&[]) {
        out.push(resolve_walk(heap, b, ns_name, &scope));
    }
    reuse_or_rebuild(heap, form, items, out)
}

/// `(%match :ctx valexpr (pattern body…) …)` — resolve `valexpr` and each clause
/// body with the clause pattern's symbols treated as bound (over-approximation:
/// all symbols anywhere in the pattern are collected, so a binder is never
/// qualified; a pinned reference there is left bare — safe, occasionally lossy).
fn resolve_match(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    if items.len() < 3 {
        return generic_resolve(heap, form, items, ns_name, locals);
    }
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // %match
    out.push(items[1]); // :ctx keyword
    out.push(resolve_walk(heap, items[2], ns_name, locals)); // value expression
    for &clause in &items[3..] {
        let cparts = match heap.list_items(clause) {
            Some(c) if c.len() >= 2 => c,
            _ => {
                out.push(clause);
                continue;
            }
        };
        let mut scope = locals.to_vec();
        collect_all_syms(heap, cparts[0], &mut scope);
        let mut cout = Vec::with_capacity(cparts.len());
        // the pattern verbatim but for its `&optional` defaults, which are expressions
        cout.push(resolve_pattern_defaults(heap, cparts[0], ns_name, &scope));
        for &b in &cparts[1..] {
            cout.push(resolve_walk(heap, b, ns_name, &scope));
        }
        out.push(reuse_or_rebuild(heap, clause, &cparts, cout));
    }
    reuse_or_rebuild(heap, form, items, out)
}

/// `pattern` with each `&optional` slot's default resolved, at any depth; everything else
/// verbatim. The defaults see the pattern's own names as bound (`scope`).
fn resolve_pattern_defaults(
    heap: &mut Heap,
    pattern: Value,
    ns_name: &str,
    scope: &[value::Symbol],
) -> Value {
    let Some(items) = heap.list_items(pattern) else {
        return pattern;
    };
    let mut optional = false;
    let mut out = Vec::with_capacity(items.len());
    for &it in &items {
        match it.unpack() {
            ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => optional = true,
            ValueRef::Sym(s) if value::symbol_is(s, kw::AMP) => optional = false,
            _ => {}
        }
        let slot = heap.list_items(it);
        out.push(match slot.as_deref() {
            Some(&[pat, default]) if optional => {
                let pat = resolve_pattern_defaults(heap, pat, ns_name, scope);
                let default = resolve_walk(heap, default, ns_name, scope);
                rebuild_list(heap, it, vec![pat, default])
            }
            _ => resolve_pattern_defaults(heap, it, ns_name, scope),
        });
    }
    reuse_or_rebuild(heap, pattern, &items, out)
}

/// Resolve every element of a list and rebuild (the fallback for binder forms whose
/// shape didn't match — never over-qualifies because it adds no bound names).
fn generic_resolve(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let mut out = Vec::with_capacity(items.len());
    for &it in items {
        out.push(resolve_walk(heap, it, ns_name, locals));
    }
    reuse_or_rebuild(heap, form, items, out)
}

/// Resolve the **default expressions** inside a param list, leaving every binder name
/// alone. `(x &optional (n *lim*))` keeps `x` and `n` verbatim but resolves `*lim*` to
/// `ns/*lim*`.
///
/// Passed through verbatim, a default expression's reference to the module's own global
/// would resolve at call time in whatever namespace the CALLER was in and raise
/// `unbound symbol`.
///
/// Earlier binders are in scope for a later default, so they accumulate as locals as the
/// list is walked. A destructuring `[a b]` param is a pattern, not a reference, and stays
/// verbatim.
fn resolve_param_defaults(
    heap: &mut Heap,
    params: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let items = match heap.list_or_vector_items(params) {
        Some(i) => i,
        None => return params,
    };
    let mut inner = locals.to_vec();
    let mut out = Vec::with_capacity(items.len());
    for item in items.iter().copied() {
        match item.unpack() {
            // `(name default…)` — the binder stays, the defaults resolve.
            ValueRef::Pair(_) => {
                let parts = heap.list_or_vector_items(item).unwrap_or_default();
                match parts.first().map(|v| v.unpack()) {
                    Some(ValueRef::Sym(binder)) => {
                        let mut rebuilt = Vec::with_capacity(parts.len());
                        rebuilt.push(parts[0]);
                        for &d in &parts[1..] {
                            rebuilt.push(resolve_walk(heap, d, ns_name, &inner));
                        }
                        inner.push(binder);
                        out.push(reuse_or_rebuild(heap, item, &parts, rebuilt));
                    }
                    _ => out.push(item),
                }
            }
            ValueRef::Sym(s) => {
                if !value::symbol_is(s, kw::AMP)
                    && !value::symbol_is(s, kw::AMP_OPTIONAL)
                    && !value::symbol_is(s, kw::AMP_REST)
                {
                    inner.push(s);
                }
                out.push(item);
            }
            _ => out.push(item),
        }
    }
    // A vector parameter list comes back a list, as it always has; only a list is reused.
    if matches!(params.unpack(), ValueRef::Vector(_)) {
        return rebuild_list(heap, params, out);
    }
    reuse_or_rebuild(heap, params, &items, out)
}

/// The names a `let` binder binds, appended to `out`: a symbol, with its base name too when it
/// carries a type annotation (`x:int` binds `x`); for a destructuring PATTERN, every symbol in
/// it — the over-approximation `resolve_match` makes, so a binder is never qualified. A
/// macro template is resolved before its patterns are lowered, and under `ns_assume_own` a
/// name missed here is qualified to the namespace and comes out unbound.
fn collect_binder_syms(heap: &Heap, binder: Value, out: &mut Vec<value::Symbol>) {
    match binder.unpack() {
        ValueRef::Sym(s) => {
            out.push(s);
            let name = value::symbol_name_ref(s);
            if let Some(i) = name.find(':').filter(|&i| i > 0) {
                out.push(value::intern(&name[..i]));
            }
        }
        _ => collect_all_syms(heap, binder, out),
    }
}

/// Collect parameter-binder symbols from a param list (mirrors `fn_params` /
/// `parse_params`): plain symbols, `(name default)` optionals' names; skips the
/// `&`/`&optional`/`&rest` markers. Appends to `out`.
fn collect_param_syms(heap: &Heap, params: Value, out: &mut Vec<value::Symbol>) {
    let items = match heap.list_or_vector_items(params) {
        Some(i) => i,
        None => return,
    };
    for item in items {
        match item.unpack() {
            ValueRef::Sym(s) => {
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST)
                {
                    continue;
                }
                collect_binder_syms(heap, item, out);
            }
            ValueRef::Pair(_) => {
                // `(name default)` — the binder is the first element.
                let inner = heap.list_or_vector_items(item).unwrap_or_default();
                if let Some(&first) = inner.first() {
                    collect_binder_syms(heap, first, out);
                }
            }
            // A destructuring parameter (`[a b]`, `{:keys [a]}`) binds every name in it.
            ValueRef::Vector(_) | ValueRef::Map(_) => collect_all_syms(heap, item, out),
            _ => {}
        }
    }
}

/// Collect every symbol appearing anywhere in `v` (used to over-approximate a
/// pattern's bound names — see `resolve_match`) — except an `&optional` slot's default,
/// `(p default)`, which is an expression: its names are references, and taken for binders
/// they were never qualified (ADR-619).
pub(super) fn collect_all_syms(heap: &Heap, v: Value, out: &mut Vec<value::Symbol>) {
    match v.unpack() {
        ValueRef::Sym(s) => out.push(s),
        ValueRef::Pair(_) => {
            if let Some(items) = heap.list_items(v) {
                let mut optional = false;
                for it in items {
                    match it.unpack() {
                        ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => {
                            optional = true
                        }
                        ValueRef::Sym(s) if value::symbol_is(s, kw::AMP) => optional = false,
                        _ => {}
                    }
                    match heap.list_items(it).as_deref() {
                        Some(&[pat, _default]) if optional => collect_all_syms(heap, pat, out),
                        _ => collect_all_syms(heap, it, out),
                    }
                }
            }
        }
        ValueRef::Vector(id) => {
            for it in heap.vector(id).to_vec() {
                collect_all_syms(heap, it, out);
            }
        }
        ValueRef::Map(id) => {
            for (k, val) in heap.map_entries(id) {
                collect_all_syms(heap, k, out);
                collect_all_syms(heap, val, out);
            }
        }
        ValueRef::Set(id) => {
            for e in heap.set_elems(id) {
                collect_all_syms(heap, e, out);
            }
        }
        _ => {}
    }
}

/// Is `after` — what the walk made of `before` — the very same value? A symbol by its
/// interned id; a list, vector, map or set by its handle, which the walk hands back only
/// when nothing inside changed. Anything else (a number, a string, a keyword) the walk
/// returns as it is, so it is the same.
fn same_value(before: Value, after: Value) -> bool {
    match (before.unpack(), after.unpack()) {
        (ValueRef::Sym(a), ValueRef::Sym(b)) => a == b,
        (ValueRef::Pair(a), ValueRef::Pair(b)) => a.0 == b.0,
        (ValueRef::Vector(a), ValueRef::Vector(b)) => a.0 == b.0,
        (ValueRef::Map(a), ValueRef::Map(b)) => a.0 == b.0,
        (ValueRef::Set(a), ValueRef::Set(b)) => a.0 == b.0,
        (
            ValueRef::Sym(_)
            | ValueRef::Pair(_)
            | ValueRef::Vector(_)
            | ValueRef::Map(_)
            | ValueRef::Set(_),
            _,
        ) => false,
        _ => true,
    }
}

/// Did the walk leave every item as it was? Equal length and each the same value.
fn all_unchanged(before: &[Value], after: &[Value]) -> bool {
    before.len() == after.len()
        && before
            .iter()
            .zip(after)
            .all(|(&item_before, &item_after)| same_value(item_before, item_after))
}

/// `form` itself when the walk changed none of its `items` (`out` is what it made of
/// them, position by position), else a rebuilt list carrying `form`'s position.
///
/// Sound because the walk's input has **one pair in one position** (KI-288: the
/// expansion walk rebuilds every list, and every rewrite after it copies what it
/// duplicates), and `resolve` has no other input. Handing a pair back puts it exactly
/// where it was, so the output keeps that property — a pair unchanged here is one the VM
/// body cache, the closure-template cache and the position tables already knew at that
/// one position, with the same contents.
fn reuse_or_rebuild(heap: &mut Heap, form: Value, items: &[Value], out: Vec<Value>) -> Value {
    if all_unchanged(items, &out) {
        return form;
    }
    rebuild_list(heap, form, out)
}

/// Rebuild a binding container preserving list-vs-vector shape (and position).
pub(super) fn rebuild_seq_like(heap: &mut Heap, original: Value, items: Vec<Value>) -> Value {
    match original.unpack() {
        ValueRef::Vector(_) => heap.alloc_vector(items),
        _ => rebuild_list(heap, original, items),
    }
}
