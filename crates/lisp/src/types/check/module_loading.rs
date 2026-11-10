//! The checker's module loading: which top-level forms it pre-evaluates (`require`, the
//! load-path setters), the feature-registry test for "is this module loaded", the
//! transitive materialisation of the modules a loaded body names, and the probe for the
//! qualified references no module could satisfy.

use super::*;

/// True when `form` is a top-level `(require …)` call — the one form the
/// checker pre-evaluates so a module's macros (e.g. `defserver` from
/// `std/proc/gen.blsp`) are resolvable for the rest of the file.
pub(super) fn is_require_form(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, "require-one");
        }
    }
    false
}

/// A top-level form that changes **where** a module is found, and so has to be
/// evaluated in pass 1 for the same reason `(require …)` is: the whole file is
/// checked before any of it runs, so a path the program sets up for itself does
/// not exist yet when the forms that depend on it are walked.
///
/// Without this, a file that puts its own directory on the path and then uses a
/// MACRO from it had that macro read as an unknown head — and an unknown head's
/// arguments are walked as if they were evaluated code, so every operand of the
/// macro was reported as an unbound symbol. Found on wos, an OS written in
/// Brood, whose `(reflect/add-load-path "compiler")` precedes an assembler macro:
/// nine false warnings per build on a file that assembles, boots and prints.
///
/// Evaluating it is what makes the checker MORE complete here, not less: with
/// the module resolved, `resolves_to_macro` recognises the head, the existing
/// macro rule leaves its arguments alone, and every *other* form in the file —
/// including the module's own definitions — is checked normally. Silencing the
/// walk instead would have traded one false positive for a blind spot.
///
/// The cost, stated plainly: `*load-path*` is runtime-wide, so it is WIDER for the
/// duration of the check. `check_forms` restores it, but a concurrent process
/// resolving a module inside that window sees the wider path. That is the same
/// property evaluating a `(require …)` already has — it loads the module globally
/// — so this extends an existing trade rather than opening a new one.
pub(super) fn is_load_path_form(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, "reflect/add-load-path")
                || crate::core::value::symbol_is(s, "reflect/set-load-path");
        }
    }
    false
}

/// Is module `name` already loaded — i.e. present in the `*features*` registry the
/// runtime's `require-one` consults? The checker's "do I need to load this `(:use …)`
/// target?" test (see `setup_check_imports`).
///
/// Deliberately the *feature* record and not "does some `name/…` global exist": a std
/// module can share its namespace with kernel primitives (`file/slurp` & co. exist with
/// no `std/file.blsp` loaded), and the presence test then reports loaded for a module
/// whose Brood-level definitions are entirely absent.
///
/// A read, never a load. `false` whenever the answer can't be read off the image
/// (no `*features*`, a non-map value) — the safe direction, since the caller's
/// response is an idempotent `require-one`.
pub(super) fn feature_loaded(heap: &mut Heap, name: &str) -> bool {
    // Allocate the key BEFORE reading the map handle: an allocation must not run
    // between reading `mid` and using it (this runs under the checker's GC block, so
    // nothing collects here, but the ordering keeps that independent of the block).
    let key = heap.alloc_string(name);
    let features = heap.env_get(heap.global(), value::intern(reg::FEATURES));
    match features {
        Some(Value::Map(mid)) => heap.map_get(mid, key).is_some(),
        _ => false,
    }
}

/// Load `mod_sym` unless it is already loaded (the FEATURE registry, exactly what the
/// runtime `require-one` consults — see `setup_check_imports` for why "some `mod/*` global
/// exists" was the wrong test). Advisory: a load error is swallowed, the checker never gates
/// on a missing module. `require-one` is idempotent, so a spurious call costs a map lookup.
pub(super) fn ensure_loaded(heap: &mut Heap, mod_sym: Symbol) {
    if feature_loaded(heap, &value::symbol_name(mod_sym)) {
        return;
    }
    let quoted = heap.list(vec![
        Value::Sym(value::intern("quote")),
        Value::Sym(mod_sym),
    ]);
    let form = heap.list(vec![Value::Sym(value::intern("require-one")), quoted]);
    let root = heap.global();
    let _ = crate::eval::eval(heap, form, root);
}

/// Materialise, transitively, every module that the LOADED modules' bodies refer to by a
/// qualified name — `text` for a loaded `editor/buffer`, whose `buffer-current-line` calls
/// `text/char->line`.
///
/// A program loads such a module on the first call that reaches the name (ADR-335's lazy
/// policy: the global-lookup miss path), and the checker never calls. The stdlib image
/// records only the edges the policy followed at build time — header `(:use …)`s and macro
/// heads — so materialising a module from it brings in none of the modules its plain function
/// references name. The checker's inference then reads a loaded body up to the first
/// unmaterialised qualified name and stops: `buffer-current-line` inferred `-> any`, though
/// `text/char->line` DECLARES `(rope int -> int)`, and every derivation above it — the whole
/// point of declaring the leaf — was lost, in `nest check` and the LSP alike.
///
/// Fixpoint: scan each loaded module's bindings once (a per-thread memo, since a project
/// check runs this per file), collect the module prefixes its closures' bodies mention, load
/// the ones not yet loaded, and go again while something new loaded. A prefix is tried once
/// per thread; a name that is not a module (a load error) is swallowed like every other
/// advisory load. Runs under the check's `EagerLoadScope`, and before the passes that infer.
///
/// `pub` for `tests/curated_names_load_nothing.rs`, which observes this scan's decision in
/// a process of its own (whether a std module is loaded is process-wide state once an image
/// is installed, so the in-crate suite cannot hold the precondition).
pub fn materialise_referenced_modules(heap: &mut Heap) {
    thread_local! {
        static SCANNED: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
        static TRIED: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
    }
    loop {
        let unscanned: Vec<Symbol> = loaded_feature_modules(heap)
            .into_iter()
            .filter(|m| SCANNED.with(|s| s.borrow_mut().insert(*m)))
            .collect();
        if unscanned.is_empty() {
            return;
        }
        // One pass over the globals for all of this round's modules.
        let prefixes: Vec<String> = unscanned
            .iter()
            .map(|m| format!("{}/", value::symbol_name(*m)))
            .collect();
        let mut wanted: HashSet<Symbol> = HashSet::new();
        let mut names: HashSet<Symbol> = HashSet::new();
        let global_env = heap.global();
        let members =
            heap.global_symbols_where(|name| prefixes.iter().any(|p| name.starts_with(p.as_str())));
        for g in members {
            let Some(Value::Fn(cid)) = heap.env_get(global_env, g) else {
                continue;
            };
            let bodies: Vec<Value> = heap
                .closure(cid)
                .arms
                .iter()
                .flat_map(|arm| arm.body.iter().copied())
                .collect();
            for body in bodies {
                collect_qualified_names(heap, body, &mut names);
            }
        }
        // ADR-370: a name whose DECLARED signature the image carries asks for no load — the
        // checker reads its type from the footer. A name without one (an undeclared
        // function, a value, a module no image describes) still brings its module in, so
        // the body walk that infers it has the leaf.
        let trace = crate::diagnostics::debug_flags::image_trace();
        let mut names: Vec<Symbol> = names.into_iter().collect();
        names.sort_by_key(|s| value::symbol_name(*s));
        for name in names {
            // A name already BOUND — a native registered under a module's prefix
            // (`string/split`, `file/stat`), a prelude binding, a loaded module's def — is
            // read from its binding; loading `std/string.blsp` for `string/split` gives the
            // checker nothing it does not have. The eager drain's own rule (KI-150). Both
            // skips sit under the one lever, so `BROOD_NO_IMAGE_SIGS=1` is exactly the
            // pre-ADR-370 whole-module scan and the differential covers both.
            //
            // A CURATED name (`sigs::curated_sig`) is read from the table ahead of the
            // footer and ahead of inference (`sig_of`'s order), so its verdict never depends
            // on the load either: `seq/filter` is curated and named by 38 std modules'
            // bodies, `io/puts` by 26, and each check materialised `seq` and `io` for a type
            // it already had.
            if crate::eval::derive::image_sigs_enabled()
                && (heap.env_get(global_env, name).is_some()
                    || crate::eval::derive::image_sig_text(heap, name).is_some()
                    || sigs::has_curated_sig(name))
            {
                continue;
            }
            let text = value::symbol_name_ref(name);
            if let Some(slash) = text.rfind('/')
                && slash > 0
            {
                let module = value::intern(&text[..slash]);
                // The insert is the LOAD SET — it must not sit behind the trace flag.
                // ADR-370's first shape wrote `trace && wanted.insert(module)`, and the
                // short-circuit made the whole transitive scan a no-op in every process
                // that did not set `BROOD_IMAGE_TRACE` (KI-171): the one test of it ran its
                // child WITH the trace. `transitive_scan_loads_without_the_trace` pins it.
                wanted.insert(module);
                // The question KI-150 took days to answer by experiment — WHICH
                // references made the check load a module — printed per name in sorted
                // order: every one is a name to declare a signature for, and the load
                // stays until the LAST of them has one.
                if trace
                    && heap.env_get(global_env, module).is_none()
                    && !feature_loaded(heap, &text[..slash])
                {
                    eprintln!("[image] check materialises {} for {text}", &text[..slash]);
                }
            }
        }
        let mut loaded_any = false;
        // Load in name order, not hash order: a load has side effects (its own warnings,
        // registry writes), and the same file must do them the same way every run (B8).
        let mut wanted: Vec<Symbol> = wanted.into_iter().collect();
        wanted.sort_by_key(|m| value::symbol_name(*m));
        for module in wanted {
            if !TRIED.with(|t| t.borrow_mut().insert(module)) {
                continue;
            }
            if feature_loaded(heap, &value::symbol_name(module)) {
                continue;
            }
            ensure_loaded(heap, module);
            loaded_any |= feature_loaded(heap, &value::symbol_name(module));
        }
        if !loaded_any {
            return;
        }
    }
}

/// The names in the `*features*` registry — every module loaded in this process.
fn loaded_feature_modules(heap: &Heap) -> Vec<Symbol> {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern(reg::FEATURES)) else {
        return Vec::new();
    };
    heap.map_entries(mid)
        .into_iter()
        .filter_map(|(k, _)| match k {
            Value::Str(id) => Some(value::intern(heap.string(id))),
            _ => None,
        })
        .collect()
}

/// Every qualified symbol anywhere in `form` — `math/quot`, `editor/buffer/point` — descending
/// through lists, vectors and maps. The root escape `/name` and a bare name contribute nothing.
/// Over-approximate on purpose (quoted data counts): the only cost of a spurious name is one
/// swallowed load of its module.
fn collect_qualified_names(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) => {
                let name = value::symbol_name_ref(s);
                if name.rfind('/').is_some_and(|slash| slash > 0) {
                    out.insert(s);
                }
            }
            Value::Pair(_) => {
                if let Some(items) = list_items(heap, v) {
                    work.extend(items);
                }
            }
            Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
            Value::Map(id) => {
                for (k, val) in heap.map_entries(id) {
                    work.push(k);
                    work.push(val);
                }
            }
            _ => {}
        }
    }
}

/// The prefixes (`mod/`) of the file's qualified references whose module no `require` could
/// find — see `Ctx::absent_ns`. Only a module the image does not know is asked about, so a
/// file whose references all resolve pays one scan of its forms and nothing else; the
/// question itself is the loader's (`%module-findable?`), so "could be required" means
/// exactly what `require` means by it: a baked-in std module, a dependency's, a project
/// source on the load path, a named project's registered module, an image section.
///
/// Quoted and quasiquoted data are not scanned (`'commands/cmd-x` in a keymap is a symbol,
/// not a reference); an alias prefix is not a module name. The file's OWN modules are
/// skipped — the ones it declares, and the ones whose source it carries as text (a string
/// holding `(defmodule name …)`, which it `%load-string`s or writes out to load: the test
/// suite's fixture modules). So is a `name.blsp` beside the file, which a script loads by
/// path (`examples/hot-reload` `reflect/load`s its `greeter.blsp`).
///
/// Asking evaluates Brood, which may collect: every handle is read before the first probe
/// and only symbols are held across it, and the caller re-reads its forms afterwards.
pub(super) fn absent_module_prefixes(
    heap: &mut Heap,
    forms: &[Value],
    expanded: &[Value],
) -> HashSet<String> {
    let known = heap.known_ns_prefixes();
    let mut own: HashSet<String> = heap
        .compile_ns()
        .map(|ns| value::symbol_name(ns).to_string())
        .into_iter()
        .collect();
    for &form in forms {
        if let Some(module) = crate::eval::macros::defmodule_form_name(heap, form) {
            own.insert(value::symbol_name(module).to_string());
            own.insert(value::symbol_name(heap.root_module_name(module)).to_string());
        }
        collect_module_texts(heap, form, &mut own);
    }
    // `prefix/` → the module a `require` of it would ask for (rooted, ADR-070).
    let mut candidates: HashMap<String, Symbol> = HashMap::new();
    for &form in forms.iter().chain(expanded) {
        collect_unknown_qualified(heap, form, &known, &own, &mut candidates);
    }
    if candidates.is_empty() {
        return HashSet::new();
    }
    let probe = value::intern("%module-findable?");
    if heap.env_get(heap.global(), probe).is_none() {
        return HashSet::new();
    }
    let beside = heap
        .current_file()
        .and_then(|file| std::path::Path::new(file).parent())
        .map(|dir| dir.to_string_lossy().into_owned());
    let root = heap.global();
    let mut probed: HashMap<Symbol, bool> = HashMap::new();
    let mut absent = HashSet::new();
    let mut candidates: Vec<(String, Symbol)> = candidates.into_iter().collect();
    candidates.sort(); // probe order is report-independent, but keep it deterministic
    for (prefix, module) in candidates {
        let findable = *probed.entry(module).or_insert_with(|| {
            // Evaluated as a form, as `expand_and_load` evaluates a file's `(require …)`:
            // a nested eval, so the check's GC block holds across it. A probe that fails
            // says nothing either way: stay silent, as before.
            let quoted = heap.list(vec![
                Value::symbol(value::intern(crate::core::keywords::QUOTE)),
                Value::symbol(module),
            ]);
            let dir = match &beside {
                Some(dir) => heap.alloc_string(dir),
                None => Value::nil(),
            };
            let call = heap.list(vec![Value::symbol(probe), quoted, dir]);
            !matches!(
                crate::eval::eval(heap, call, root),
                Ok(Value::Bool(false) | Value::Nil)
            )
        });
        if !findable {
            absent.insert(prefix);
        }
    }
    absent
}

/// Add to `out` the name of every module whose source `form` carries as text: each
/// `(defmodule name` inside a string literal, anywhere in the form, quoted data included.
fn collect_module_texts(heap: &Heap, form: Value, out: &mut HashSet<String>) {
    const OPENER: &str = "(defmodule ";
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form {
        Value::Str(id) => {
            let text = heap.string(id);
            let mut rest = text;
            while let Some(at) = rest.find(OPENER) {
                rest = rest[at + OPENER.len()..].trim_start();
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == ')' || c == '"')
                    .unwrap_or(rest.len());
                if end > 0 {
                    out.insert(rest[..end].to_string());
                }
            }
        }
        Value::Pair(_) => {
            for item in list_items(heap, form).unwrap_or_default() {
                collect_module_texts(heap, item, out);
            }
        }
        Value::Vector(id) => {
            for item in heap.vector(id).to_vec() {
                collect_module_texts(heap, item, out);
            }
        }
        Value::Map(id) => {
            for (key, val) in heap.map_entries(id) {
                collect_module_texts(heap, key, out);
                collect_module_texts(heap, val, out);
            }
        }
        _ => {}
    })
}

/// Add to `out` every qualified symbol in `form` (outside quoted data) whose `mod/` prefix
/// the image does not know, keyed by that prefix, with the module a load would ask for.
fn collect_unknown_qualified(
    heap: &Heap,
    form: Value,
    known: &HashSet<String>,
    own: &HashSet<String>,
    out: &mut HashMap<String, Symbol>,
) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form {
        Value::Sym(s) => {
            let name = value::symbol_name_ref(s);
            let Some(slash) = name.rfind('/') else {
                return;
            };
            if slash == 0 || slash + 1 == name.len() {
                return;
            }
            let prefix = &name[..=slash];
            if known.contains(prefix) || out.contains_key(prefix) {
                return;
            }
            let Some(module) = crate::eval::derive::module_to_require(heap, s) else {
                return;
            };
            let module_name = value::symbol_name_ref(module);
            // A package roots its modules (ADR-070): `pkg/shapes` is the `shapes` it declares.
            let last_segment = module_name.rsplit('/').next().unwrap_or(module_name);
            if !own.contains(module_name) && !own.contains(last_segment) {
                out.insert(prefix.to_string(), module);
            }
        }
        Value::Pair(_) => {
            let Some(items) = list_items(heap, form) else {
                return;
            };
            if let Some(Value::Sym(head)) = items.first()
                && (value::symbol_is(*head, crate::core::keywords::QUOTE)
                    || value::symbol_is(*head, crate::core::keywords::QUASIQUOTE))
            {
                return;
            }
            for item in items {
                collect_unknown_qualified(heap, item, known, own, out);
            }
        }
        Value::Vector(id) => {
            for item in heap.vector(id).to_vec() {
                collect_unknown_qualified(heap, item, known, own, out);
            }
        }
        Value::Map(id) => {
            for (key, val) in heap.map_entries(id) {
                collect_unknown_qualified(heap, key, known, own, out);
                collect_unknown_qualified(heap, val, known, own, out);
            }
        }
        _ => {}
    })
}
