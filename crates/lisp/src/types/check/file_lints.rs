//! The whole-file lint passes that run around the body walk: a name defined twice in one
//! module, the declaration and definition conformance passes, the shape lints that read a
//! form's structure rather than its types, unused `(:use …)` imports, and the report of a
//! per-file budget the checker exhausted.

use super::*;

/// Warn when one file binds the same top-level name twice in one module: the
/// later `def`/`defn`/`defn-`/`defmacro`/`defdyn`/`defrecord` silently replaces the
/// earlier, so the first is dead code that still reads as the definition. Found the hard
/// way: `project.blsp` carried a public sorted `source-files` and, 1,300 lines later, a
/// private unsorted one — every caller ran the second while the first's docstring made
/// the sortedness promise. Nothing said so: the cross-file duplicate-global lint is
/// per-namespace across files, and one file's rebinding of its own name is legal.
///
/// Reads the *un-expanded* forms, descending `(do …)` and `(check-allow …)` wrappers the
/// way the sig collector does. Region-aware (ADR-223): a `(defmodule …)` starts a fresh
/// name set, so two modules in one file may each bind `helper`. Suppress a deliberate
/// override with `(check-allow :duplicate-def …)` around the second definition.
pub(super) fn lint_duplicate_defs(heap: &mut Heap, forms: &[Value], out: &mut Vec<Finding>) {
    let mut seen: HashMap<Symbol, Option<Pos>> = HashMap::new();
    for &form in forms {
        lint_duplicate_defs_in(heap, form, false, &mut seen, out);
    }
}

fn lint_duplicate_defs_in(
    heap: &mut Heap,
    form: Value,
    allowed: bool,
    seen: &mut HashMap<Symbol, Option<Pos>>,
    out: &mut Vec<Finding>,
) {
    // Recurses through nested `(do (do …))`, like `collect_register_sig_forms` beside it,
    // and for the same reason grows the stack in heap-backed segments: a 30 000-deep
    // legal chain (`checker_survives_pathologically_deep_forms`) overflowed the native
    // stack here — the one walker of the un-expanded tree that lacked the guard, and a
    // SIGSEGV `catch_unwind` cannot catch.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        lint_duplicate_defs_in_inner(heap, form, allowed, seen, out)
    })
}

fn lint_duplicate_defs_in_inner(
    heap: &mut Heap,
    form: Value,
    allowed: bool,
    seen: &mut HashMap<Symbol, Option<Pos>>,
    out: &mut Vec<Finding>,
) {
    let Some(items) = heap.list_items(form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    if value::symbol_is(head, kw::DEFMODULE) {
        seen.clear();
        return;
    }
    if value::symbol_is(head, kw::DO) {
        for &it in &items[1..] {
            lint_duplicate_defs_in(heap, it, allowed, seen, out);
        }
        return;
    }
    if value::symbol_is(head, "check-allow") || value::symbol_is(head, "%lint-allow") {
        let here = allowed
            || matches!(items.get(1), Some(&Value::Keyword(k)) if value::symbol_is(k, "duplicate-def"));
        for &it in &items[2..] {
            lint_duplicate_defs_in(heap, it, here, seen, out);
        }
        return;
    }
    let is_def = [
        kw::DEF,
        kw::DEFN,
        kw::DEF_PRIVATE,
        kw::DEFN_PRIVATE,
        kw::DEFMACRO,
        kw::DEFDYN,
        kw::DEFRECORD,
    ]
    .iter()
    .any(|k| value::symbol_is(head, k));
    if !is_def {
        return;
    }
    let Some(&Value::Sym(name)) = items.get(1) else {
        return;
    };
    let pos = heap.form_pos_only(form);
    match seen.get(&name) {
        Some(first) if !allowed => {
            let where_first = first
                .map(|p| format!(" at line {}", p.line))
                .unwrap_or_default();
            out.push(Finding::new(
                Lint::DuplicateDef,
                pos,
                format!(
                    "`{}` is defined twice in this file: this definition replaces the one{where_first}, \
                     which is now dead code — remove one, or wrap a deliberate override in \
                     (check-allow :duplicate-def …)",
                    value::symbol_name(name)
                ),
            ));
        }
        _ => {
            seen.insert(name, pos);
        }
    }
}

/// The declaration and definition conformance passes: protocol and behaviour impls,
/// ability call sites and their `:requires` and op collisions, multimethod calls with no
/// method (ADR-179), and each ability impl's return against the op's declared `:-> RET`.
pub(super) fn check_conformance(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ability_info: &protocol::AbilityInfo,
    multi_info: &protocol::MultiInfo,
    ctx: &Ctx,
    out: &mut Vec<Finding>,
) {
    let protocols = protocol::collect(heap, forms);
    protocol::check_impls(heap, forms, &protocols, out);
    protocol::check_behaviours(heap, forms, expanded, &protocols, out);
    protocol::check_dispatch_calls(heap, expanded, ability_info, multi_info, out);
    protocol::check_sealed(ability_info, out);
    protocol::check_requires(ability_info, out);
    // Two abilities declaring the same op name would clobber each other's generic
    // function (ADR-172).
    protocol::check_op_collisions(ability_info, out);
    walk::check_impl_returns(heap, expanded, ctx, out);
}

/// The lints that read a form's shape rather than its types, after the body walk: non-tail
/// self-recursion, sealed-`match` exhaustiveness (ADR-187), effects in a `:when` guard, a
/// `catch` that discards its error, a load-time `def` of another module's global, and
/// declared `:pure`/`:total` properties (ADR-351).
/// The `match`, guard and `catch` lints read the UN-expanded forms — those shapes survive
/// only before expansion — and position each warning at the form the reader positioned.
pub(super) fn run_shape_lints(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ctx: &Ctx,
    out: &mut Vec<Finding>,
) {
    for &form in expanded {
        recursion::check_recursion(heap, form, out);
    }
    exhaustive::check_matches(heap, forms, ctx, out);
    guard_effects::check_guards(heap, forms, out);
    discarded_catch::check_discarded_catches(heap, forms, out);
    deprecated_macros::check_deprecated_macros(heap, forms, out);
    let own: Vec<String> = crate::eval::macros::file_modules(heap, forms)
        .into_iter()
        .map(value::symbol_name)
        .collect();
    cross_module_def::check_cross_module_defs(heap, expanded, &own, out);
    properties::check_properties(heap, expanded, ctx, out);
}
/// Unused `(:use …)` imports: a clause whose module contributed importable names, none of
/// which the file references. The module names come from the UN-expanded header (the
/// clause is gone after expansion). Unused module-private `defn`s are a whole-project
/// check (`project-unused-private-warnings`), since other files reach them qualified.
pub(super) fn lint_unused_imports(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    out: &mut Vec<Finding>,
) {
    // A file with an unresolved BARE reference has an import table that doesn't cover
    // everything it names, so "module M contributed nothing you reference" is not
    // provable — the unresolved name may be M's — and the advice would be to delete an
    // import the code needs. The lint stands down whenever a bare name was unbound.
    let unresolved_bare = out.iter().any(|finding| {
        finding
            .message
            .strip_prefix("unbound symbol: ")
            .is_some_and(|rest| !rest.split(' ').next().unwrap_or("").contains('/'))
    });
    let use_modules = extract_use_module_names(heap, forms);
    if use_modules.is_empty() || unresolved_bare {
        return;
    }
    // Both trees: a MACRO head (`defserver` from `gen`) is gone from the expanded forms,
    // so a module imported for its macros would read as unused. Counting a name expansion
    // discards can only SUPPRESS a warning — the safe direction for this lint.
    let mut all_refs = collect_all_syms(heap, expanded);
    all_refs.extend(collect_all_syms(heap, forms));
    // Each module's contributed names: the qualified name AND the local alias the file
    // calls, since a `:use` import is normally referenced unqualified.
    let mut module_names: HashMap<String, Vec<Symbol>> = HashMap::new();
    for (local, qualified) in &heap.imported_pairs() {
        let qualified_name = value::symbol_name(*qualified);
        if let Some(slash) = qualified_name.rfind('/') {
            let names = module_names
                .entry(qualified_name[..slash].to_string())
                .or_default();
            names.push(*local);
            names.push(*qualified);
        }
    }
    // A module reached by a qualified `mod/name` anywhere (including its private names,
    // which aren't imported at all) counts its `:use` as load-bearing.
    let qualified_modules: HashSet<String> = all_refs
        .iter()
        .filter_map(|&symbol| {
            let name = value::symbol_name(symbol);
            name.rfind('/').map(|slash| name[..slash].to_string())
        })
        .collect();
    for module_name in &use_modules {
        // Only a module in the import table is actionable — a failed require, a
        // macro-only or an empty module is silent.
        if let Some(names) = module_names.get(module_name) {
            let used = names.iter().any(|symbol| all_refs.contains(symbol))
                || qualified_modules.contains(module_name);
            if !used {
                out.push(Finding::new(
                    Lint::UnusedImport,
                    None,
                    format!("unused :use import: {module_name}"),
                ));
            }
        }
    }
}

/// The per-file budgets that DECLINE rather than answer: past the specialization fuel
/// every re-typing question answers "unknown", and past the expression depth cap a nest
/// types as unknown. Both are sound, and both are reported, so a file the checker largely
/// gave up on does not read as checked.
pub(super) fn report_exhausted_budgets(out: &mut Vec<Finding>) {
    if sigs::fuel_exhausted() {
        out.push(Finding::new(
            Lint::GaveUp,
            None,
            format!(
                "checker gave up: the specialization budget ({} re-typings) ran out — every later call-site specialization in this file read as unknown",
                sigs::MAX_SPECIAL_FUEL
            ),
        ));
    }
    let depth_hits = infer::expr_ty_depth_hits();
    if depth_hits > 0 {
        out.push(Finding::new(
            Lint::GaveUp,
            None,
            format!(
                "checker gave up: {depth_hits} expression{} nested past the depth cap ({}) read as unknown",
                if depth_hits == 1 { "" } else { "s" },
                infer::MAX_EXPR_TY_DEPTH
            ),
        ));
    }
}
