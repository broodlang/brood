use super::numeric::{arg, expect_string};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::reader;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // macros
    primitives.def(
        "macroexpand-1",
        Arity::exact(1),
        Sig::new(vec![any], any),
        &["form"],
        "Expand form by a single macro step.",
        macroexpand_1,
    );
    // advisory type checker (the Ty lattice's first consumer; see docs/types.md)
    primitives.def(
        "%check",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        &["form"],
        "Advisory type-check a quoted form: a list of warning strings, or nil. Never raises.",
        check_builtin,
    );
    primitives.def(
        "%check-string-here",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        &["src"],
        "Advisory type-check the source string `src` under THIS process's compile context — the namespace last opened and its imports, as `eval` would resolve it — returning `{:line :col :message}` maps like `%check-string-structured`; `()` when `src` does not parse. Forms that open a `(defmodule …)` of their own are checked under it instead.",
        check_string_here);
    primitives.def(
        "%check-file",
        Arity::range(1, 2),
        // 2nd arg (optional required-mods) is a list OR vector of module names — `any`
        // so a vector closure doesn't trip the arg-type lint on our own callers.
        Sig::with_rest(vec![string], any, list_ty),
        &["path", "&optional required-mods"],
        "Advisory type-check every top-level form in the file at path: a list of `path:line:col: warning: …` strings, or nil. Does not evaluate the file. `required-mods` is the file's transitive require-closure (module-name strings) — the KI-17 reachability set that flags a qualified `mod/name` whose module the file never requires; omit it (single-file / editor) to disable that lint.",
        check_file_builtin);
    primitives.def(
        "%file-signatures",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        &["path"],
        "The signature the checker holds for every function the file at path defines: a list of `{:name :sig :declared? :informative?}` maps, `:sig` written in source syntax (`\"(int int -> int)\"`) and ready to paste into a `(sig …)`, or nil where the type names a runtime kind the grammar cannot write. `:declared?` marks the ones a `(sig …)` already states; `:informative?` marks the ones saying something an all-`any` arrow does not. Does not evaluate the file. The bulk counterpart of the editor's declare-sig action.",
        file_signatures_builtin);
    primitives.def(
        "%source-signatures",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        &["src"],
        "`%file-signatures` for source TEXT rather than a file: the checker's signature for every function `src` defines, as `{:name :sig :declared? :informative?}` maps. `()` when `src` doesn't parse, so a live editor buffer never errors mid-edit. The question `%expr-type` cannot answer — a `(defn …)` form evaluates to its own name, so the type of its VALUE says nothing about the function.",
        source_signatures);
    primitives.def(
        "%check-file-structured",
        Arity::range(1, 2),
        Sig::with_rest(vec![string], any, list_ty),
        &["path", "&optional required-mods"],
        "Like check-file but returns a list of `{:file :line :col :message}` maps instead of GNU-format strings — for tools (the `nest mcp` `check` tool, editor diagnostics). `required-mods`: see check-file.",
        check_file_structured);
    primitives.def(
        "%check-file-deps",
        Arity::range(1, 2),
        Sig::with_rest(vec![string], any, any),
        &["path", "&optional required-mods"],
        "Incremental-cache check (ADR-119): returns [warnings dep-keys fingerprint] — the GNU warning strings, the set of global observations the check made, and a fingerprint of them against the current image. Store dep-keys+fingerprint; reuse warnings on a later run iff (check-deps-fp dep-keys) still matches and the file's mtime is unchanged. `required-mods`: see check-file.",
        check_file_deps);
    primitives.def(
        "%module-direct-requires",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["path"],
        "Parse the file at path (no eval) and return `{:module <name-or-nil> :requires [<module-name> …]}` — its own module name and the modules it directly `:use`s / `:use-internals`. The edge list `project.blsp` closes transitively into each file's check-file reachability set (KI-17).",
        module_direct_requires);
    primitives.def(
        "%check-strict?",
        Arity::exact(0),
        Sig::new(vec![], bool_ty),
        &[],
        "Is STRICT checking on for this process (`nest check --strict`, or BROOD_CHECK_STRICT=1)? A verdict depends on the mode that produced it, so the incremental check cache keys its manifest on this — without it a plain run's cached verdicts are reused by a strict one, and the strict gate silently reports less than it found.",
        check_strict,
    );
    primitives.def(
        "%check-strict!",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["on?"],
        "Set STRICT checking for this process and return the new value. The setter behind the Brood-implemented `nest check --strict` (ADR-322): the mode is a process-wide flag the checker reads, so the command line flips it before the first file is checked.",
        check_strict_set,
    );
    primitives.def(
        "%check-deps-fp",
        Arity::exact(1),
        Sig::new(vec![any], string),
        &["dep-keys"],
        "Recompute the fingerprint of a file's dep-keys (from check-file-deps) against the current global image. The incremental check cache reuses a file's warnings iff this equals the stored fingerprint.",
        check_deps_fp);
    primitives.def(
        "%check-string-structured",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        &["src"],
        "Advisory type-check the source string `src`, returning a list of `{:line :col :message}` maps (1-based positions), or `()` when `src` doesn't parse (e.g. incomplete input) — the string-source counterpart of check-file-structured, for live editor-buffer diagnostics.",
        check_string_structured);
    primitives.def(
        "%expr-type",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        &["src"],
        "The type the advisory checker infers for the FIRST form in source string `src`, written the way a `sig` is (\"int\", \"(list any)\", \"(int -> int)\"), or nil when `src` doesn't parse or the checker has no opinion. reflect/type-signature answers this for a NAMED global; this answers it for an anonymous expression you just typed, which is what a REPL or a scratch/playground buffer has. Nil rather than an error on unparsable input, like %check-string-structured — both get read from a buffer that is mid-edit half the time.",
        expr_type);
    // `defn-`/`def-` emit it to record the defined name as module-private (ADR-146).
    primitives.def(
        "%register-meta",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        &["name", "clauses"],
        "Record a global's stability metadata (ADR-283) from a flat `:key value` list — `:since`/`:deprecated`/`:beta` take a version or reason string, `:use` a replacement symbol. The primitive behind the `(meta …)` form; unknown keys are ignored so a newer clause degrades on an older runtime. Cleared by any redefinition of the name, like privacy.",
        register_meta);
    primitives.def(
        "%meta-of",
        Arity::exact(1),
        Sig::new(vec![any], any),
        &["name"],
        "The stability metadata a `(meta …)` recorded for `name`, as `{:since :deprecated :use :beta}` with absent facts omitted — nil if none. `name` is resolved to the CURRENT namespace, exactly as a `def` head is, so pass a qualified symbol when asking from anywhere but the defining module.",
        meta_of);
}

// ---------- macros ----------

pub(super) fn macroexpand_1(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let (expanded, _) = crate::eval::macros::macroexpand_1(heap, arg(args, 0), env)?;
    Ok(expanded)
}
// `macroexpand` is now a Brood prelude fn over `macroexpand-1` (ADR-064).

/// `(check 'form)` — run the advisory type checker over `form` (macro-expanded
/// first, like the real compile pass) and return a list of warning strings, or
/// `nil` when nothing is provably wrong. Advisory only: it never raises.
pub(super) fn check_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    let warnings = crate::types::check::check_form(heap, form);
    let mut out = Vec::with_capacity(warnings.len());
    for w in &warnings {
        out.push(heap.alloc_string(w));
    }
    Ok(heap.list(out))
}

/// `(check-file path)` — run the advisory type checker over every top-level
/// form in the file at `path` and return a list of pre-formatted warning
/// strings (each `"path:line:col: warning: message"`), or `nil` if clean.
///
/// Reads but does **not** evaluate the file — same `check_file` walk the
/// `brood --check` CLI uses, with the file-globals accumulator threaded
/// across top-level forms. The whole-file-at-once shape is what lets `(defn
/// foo …)` at line 1 silence the unbound check on `(foo …)` at line 100. Used
/// by `(check)` in `std/tool/project.blsp` for the `nest test` / `nest run`
/// pre-flight.
pub(super) fn check_file_builtin(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("check-file: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let required = required_mods_arg(heap, arg(args, 1));
    let warnings = crate::types::check::check_file_ext(heap, &just_forms, &required);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos, msg) in &warnings {
        let s = match pos {
            Some(p) => format!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
            None => format!("{}: warning: {}", path, msg),
        };
        out.push(heap.alloc_string(&s));
    }
    Ok(heap.list(out))
}

/// `(file-signatures path)` — the signature the checker holds for every function the
/// file at `path` defines, as `{:name :sig :declared?}` maps where `:sig` is the type
/// written in **source syntax** (`"(int int -> int)"`), ready to paste into a `(sig …)`.
///
/// The bulk counterpart of the LSP's "declare sig" code action, and the reason both
/// exist: `sig` adoption across a 2800-definition standard library is the type system's
/// longest-standing backlog item, and doing it by hand means guessing what the checker
/// already knows. A signature it *cannot* write — one naming a runtime kind the grammar
/// has no word for — is reported with `:sig nil` rather than a wrong string.
///
/// Reads but does not evaluate, exactly like `check-file`.
pub(super) fn file_signatures_builtin(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-signatures", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("file-signatures: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    Ok(signatures_of(heap, &just_forms))
}

/// `(source-signatures src)` — [`file_signatures_builtin`] for source text that is not a
/// file. Same maps, same checker pass; `()` when `src` doesn't parse.
///
/// The case a file cannot cover: an editor buffer mid-edit, or a single form a live
/// evaluator just ran. `%expr-type` answers for an *expression*, whose type is the type
/// of its value — but a `(defn …)` form evaluates to its own name, so asking it what the
/// FUNCTION is typed as has no answer to give. This does, for every definition in the
/// text, and shares one code path with the file variant so a buffer and the file it will
/// be saved as can never disagree.
pub(super) fn source_signatures(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "source-signatures", arg(args, 0))?;
    let forms = match reader::read_all_positioned(heap, &src) {
        Ok(fs) => fs,
        // unparsable (e.g. mid-edit) — no signatures rather than an error, matching
        // `%check-string-structured` and `%expr-type`
        Err(_) => return Ok(heap.list(Vec::new())),
    };
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    Ok(signatures_of(heap, &just_forms))
}

/// The `{:name :sig :declared? :informative?}` list both signature builtins return.
fn signatures_of(heap: &mut Heap, forms: &[Value]) -> Value {
    let signatures = crate::types::check::file_signatures(heap, forms);
    let mut out = Vec::with_capacity(signatures.len());
    for signature in &signatures {
        let name = heap.alloc_string(&signature.name);
        let rendered = match signature.sig.to_source() {
            Some(text) => heap.alloc_string(&text),
            None => Value::Nil,
        };
        let entry = heap.map_from_pairs(vec![
            (Value::Keyword(value::intern("name")), name),
            (Value::Keyword(value::intern("sig")), rendered),
            (
                Value::Keyword(value::intern("declared?")),
                Value::Bool(signature.declared),
            ),
            // Whether the signature says anything a reader doesn't already have.
            // Decided here, on the types, because the rendered string cannot be
            // tested for it — `(string any -> any)` contains the text of the
            // uninformative `(any -> any)` and is not uninformative at all.
            (
                Value::Keyword(value::intern("informative?")),
                Value::Bool(
                    signature.sig.params.iter().any(|p| !p.is_any()) || !signature.sig.ret.is_any(),
                ),
            ),
        ]);
        out.push(entry);
    }
    heap.list(out)
}

/// `(%register-meta 'name (list :since "0.9.0" :deprecated "0.14.0" :use 'other :beta "why"))`
/// — record a global's stability metadata (ADR-283). The primitive behind the `(meta …)`
/// form, the same shape `%mark-private` and `%register-sig` have: a fact recorded against a
/// name at definition time, read back by the checker and the doc tooling.
///
/// Unknown keys are ignored rather than an error, so a newer `(meta …)` clause read by an
/// older runtime degrades to "records less" instead of failing the load.
pub(super) fn register_meta(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let Value::Sym(name) = arg(args, 0) else {
        return Err(LispError::type_err("%register-meta: name must be a symbol"));
    };
    // Qualify to the current namespace exactly as a `def` head is, via the same entry
    // `%register-sig` and `%mark-private` use. Without this the fact is keyed by the BARE
    // symbol while `env_define` clears the QUALIFIED one, so a redefinition inside a module
    // leaves the old `:deprecated` attached — which is the one rule this feature must not
    // get wrong, and which a test caught immediately.
    let name = crate::eval::macros::resolve_reference(heap, name);
    let items = list_or_vec_items(heap, arg(args, 1));
    let mut meta = crate::core::heap::NameMeta::default();
    for pair in items.chunks(2) {
        let (Some(&Value::Keyword(k)), Some(&v)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        let text = |v: Value| match v.unpack() {
            value::ValueRef::Str(id) => Some(heap.string(id).to_string()),
            _ => None,
        };
        if value::symbol_is(k, "since") {
            meta.since = text(v);
        } else if value::symbol_is(k, "deprecated") {
            meta.deprecated = text(v);
        } else if value::symbol_is(k, "beta") {
            meta.beta = text(v);
        } else if value::symbol_is(k, "use") {
            if let Value::Sym(s) = v {
                meta.use_instead = Some(s);
            }
        }
    }
    heap.set_name_meta(name, meta);
    Ok(Value::Sym(name))
}

/// `(%meta-of 'name)` — the metadata a `(meta …)` recorded, as
/// `{:since :deprecated :use :beta}` with absent facts omitted, or nil for a name with none.
pub(super) fn meta_of(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let Value::Sym(name) = arg(args, 0) else {
        return Ok(Value::Nil);
    };
    // Same resolution as the register side, so `(%meta-of 'name)` inside a module finds
    // what `(meta name …)` there recorded.
    let name = crate::eval::macros::resolve_reference(heap, name);
    let Some(meta) = heap.name_meta(name) else {
        return Ok(Value::Nil);
    };
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    for (key, text) in [
        ("since", &meta.since),
        ("deprecated", &meta.deprecated),
        ("beta", &meta.beta),
    ] {
        if let Some(t) = text {
            let v = heap.alloc_string(t);
            pairs.push((Value::Keyword(value::intern(key)), v));
        }
    }
    if let Some(s) = meta.use_instead {
        pairs.push((Value::Keyword(value::intern("use")), Value::Sym(s)));
    }
    Ok(heap.map_from_pairs(pairs))
}

/// A list-or-vector argument flattened to a `Vec<Value>`; empty for anything else.
fn list_or_vec_items(heap: &Heap, v: Value) -> Vec<Value> {
    match v.unpack() {
        value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => {
            let mut out = Vec::new();
            let mut cur = v;
            while let Value::Pair(p) = cur {
                let (h, t) = heap.pair(p);
                out.push(h);
                cur = t;
            }
            out
        }
    }
}

/// A `required-mods` argument (a list/vector of module-name strings or symbols) → a
/// `Vec<String>`. `nil` / absent → empty. Backs the optional KI-17 reachability set on
/// the `check-file*` builtins.
fn required_mods_arg(heap: &Heap, v: Value) -> Vec<String> {
    // Flatten to a Vec<Value> first (list or vector), then read each element's name —
    // keeps the heap borrows non-overlapping.
    let items: Vec<Value> = match v {
        Value::Vector(vid) => heap.vector(vid).iter().copied().collect(),
        _ => {
            let mut acc = Vec::new();
            let mut cur = v;
            while let Value::Pair(p) = cur {
                let (car, cdr) = heap.pair(p);
                acc.push(car);
                cur = cdr;
            }
            acc
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::Str(id) => out.push(heap.string(id).to_string()),
            Value::Sym(s) => out.push(value::symbol_name(s)),
            _ => {}
        }
    }
    out
}

/// `(%module-direct-requires path)` — the file's own module name and the modules it
/// directly `:use`s / `(require 'M)`s, as `{:module <name-or-nil> :requires [<name> …]}`.
/// `std/tool/project.blsp` builds the require graph from these and closes it transitively
/// into each file's `check-file` reachability set (KI-17). Reads, never evaluates.
pub(super) fn module_direct_requires(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%module-direct-requires", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!(
            "%module-direct-requires: cannot read {}: {}",
            path, e
        ))
        .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let (own, deps) = crate::types::check::module_direct_requires(heap, &just_forms);
    // No GC safepoint fires inside a single builtin, so these handles stay live without
    // rooting (same discipline as `check_file_structured`).
    let dep_vals: Vec<Value> = deps.iter().map(|d| heap.alloc_string(d)).collect();
    let requires_val = heap.alloc_vector(dep_vals);
    let module_val = match own {
        Some(n) => heap.alloc_string(&n),
        None => Value::Nil,
    };
    let module_kw = Value::keyword(value::intern("module"));
    let requires_kw = Value::keyword(value::intern("requires"));
    Ok(heap.map_from_pairs(vec![(module_kw, module_val), (requires_kw, requires_val)]))
}

/// `(check-file-deps path)` — the incremental-cache counterpart of `check-file`
/// (ADR-119 Phase 2). Returns a 3-vector `[warnings dep-keys fingerprint]`:
///   - `warnings`: the same GNU `path:line:col: warning: …` string list as `check-file`,
///   - `dep-keys`: the serializable set of global observations the check made
///     (`{:syms :kns :exp :proto}`) — store it, then re-fingerprint on a later run,
///   - `fingerprint`: a stamp of those observations against the CURRENT image; a
///     later run whose `(check-deps-fp dep-keys)` still equals this (and whose file
///     mtime is unchanged) may reuse `warnings` verbatim.
pub(super) fn check_file_deps(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file-deps", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("check-file-deps: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    // check_file_with_deps may `eval` `(require …)` (a GC safepoint) internally, but
    // returns before we allocate the result — the allocations below don't hit a
    // safepoint, so `dep_keys`/`fp_val`/`warns` stay live without extra rooting
    // (same discipline as `check_file_builtin`).
    let required = required_mods_arg(heap, arg(args, 1));
    let (warnings, dep_keys) =
        crate::types::check::check_file_with_deps_ext(heap, &just_forms, &required);
    let fp = crate::types::check::deps_fingerprint(heap, dep_keys);
    let fp_val = heap.alloc_string(&fp);
    let mut warn_vals = Vec::with_capacity(warnings.len());
    for (pos, msg) in &warnings {
        let s = match pos {
            Some(p) => format!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
            None => format!("{}: warning: {}", path, msg),
        };
        warn_vals.push(heap.alloc_string(&s));
    }
    let warns_list = heap.list(warn_vals);
    Ok(heap.alloc_vector(vec![warns_list, dep_keys, fp_val]))
}

/// `(check-deps-fp dep-keys)` — recompute the fingerprint of a file's `dep-keys`
/// (from `check-file-deps`) against the CURRENT global image. The incremental
/// cache reuses a file's warnings iff this equals the stored fingerprint (and the
/// file's mtime is unchanged). A pure read of the image — no allocation of Brood
/// values beyond the returned string.
pub(super) fn check_deps_fp(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let dep_keys = arg(args, 0);
    let fp = crate::types::check::deps_fingerprint(heap, dep_keys);
    Ok(heap.alloc_string(&fp))
}

/// `(check-strict?)` — is strict checking on for this process? The incremental check
/// cache reads it to key its manifest: a stored verdict is only reusable by a run in the
/// mode that produced it, and without the key a plain `nest check` poisons the cache for
/// the next `nest check --strict`, which then reports what the plain run found.
pub(super) fn check_strict(_args: &[Value], _env: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::Bool(crate::types::strict_checking()))
}

/// `(%check-strict! on?)` — set strict checking for this process (ADR-322). The setter the
/// Brood-implemented `nest check --strict` needs: the mode is a process-wide kernel flag read
/// by `check_file_mode`, not an argument, so the command line has to be able to flip it
/// before the first file is checked. Returns the new value.
pub(super) fn check_strict_set(args: &[Value], _env: EnvId, _heap: &mut Heap) -> LispResult {
    let on = crate::eval::truthy(arg(args, 0));
    crate::types::set_strict_checking(on);
    Ok(Value::Bool(on))
}

/// `(check-file-structured path)` — the data-shaped counterpart of
/// `check-file`. Returns a list of `{:file :line :col :message}` maps (or
/// `{:file :message}` for warnings without a position — the advisory
/// checker doesn't carry spans through macroexpansion yet, ADR-024). Used
/// by the `nest mcp` `check` tool (step 1c-a) and any other consumer that
/// wants structured diagnostics rather than a GNU-line string to re-parse.
pub(super) fn check_file_structured(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file-structured", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!(
            "check-file-structured: cannot read {}: {}",
            path, e
        ))
        .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let required = required_mods_arg(heap, arg(args, 1));
    let warnings = crate::types::check::check_file_ext(heap, &just_forms, &required);
    let file_kw = Value::keyword(value::intern("file"));
    let line_kw = Value::keyword(value::intern("line"));
    let col_kw = Value::keyword(value::intern("col"));
    let msg_kw = Value::keyword(value::intern("message"));
    let file_val = heap.alloc_string(&path);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos_opt, msg) in &warnings {
        let msg_val = heap.alloc_string(msg);
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(4);
        entries.push((file_kw, file_val));
        if let Some(p) = pos_opt {
            entries.push((line_kw, Value::int(p.line as i64)));
            entries.push((col_kw, Value::int(p.col as i64)));
        }
        entries.push((msg_kw, msg_val));
        out.push(heap.map_from_pairs(entries));
    }
    Ok(heap.list(out))
}

/// `(%expr-type src)` — the type the advisory checker infers for the FIRST form in
/// `src`, rendered the way a `sig` is written (`"int"`, `"(list any)"`,
/// `"(int -> int)"`), or nil when `src` doesn't parse or the checker has no opinion.
///
/// The checker computes this for every expression it walks in order to produce its
/// warnings; nothing could ask it for one. `%type-signature` answers the question for a
/// NAMED global, which leaves the case a REPL or a playground actually has — an anonymous
/// expression you just typed, whose name is nothing. Nil rather than an error for
/// unparsable input, matching `%check-string-structured`: both are read live, from a
/// buffer that is mid-edit half the time.
pub(super) fn expr_type(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "expr-type", arg(args, 0))?;
    let forms = match reader::read_all_positioned(heap, &src) {
        Ok(fs) => fs,
        // unparsable (e.g. mid-edit) — no opinion rather than an error
        Err(_) => return Ok(Value::nil()),
    };
    let Some((form, _)) = forms.into_iter().next() else {
        return Ok(Value::nil());
    };
    match crate::types::check::expr_ty_of(heap, form) {
        Some(ty) => {
            let rendered = ty.to_string();
            Ok(heap.alloc_string(&rendered))
        }
        None => Ok(Value::nil()),
    }
}

/// `(check-string-structured src)` — the source-string counterpart of
/// `check-file-structured`: advisory type-check the Brood source string `src` and
/// return a list of `{:line :col :message}` maps (1-based positions; no `:file`).
/// Returns `()` when `src` doesn't parse — e.g. incomplete input while an editor
/// buffer is mid-edit — so a live diagnostics loop never errors on an unbalanced
/// buffer; warnings reappear once it parses. Reuses the same checker as the file
/// variant (`types::check::check_file`).
pub(super) fn check_string_structured(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "check-string-structured", arg(args, 0))?;
    check_string_as(heap, &src, false)
}

/// `%check-string-here`: [`check_string_structured`] under the CURRENT compile context —
/// what a REPL or an editor's eval-in-buffer wants, since that is how the form is about
/// to be resolved (`types::check::check_forms_here`).
pub(super) fn check_string_here(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "check-string-here", arg(args, 0))?;
    check_string_as(heap, &src, true)
}

fn check_string_as(heap: &mut Heap, src: &str, here: bool) -> LispResult {
    let forms = match reader::read_all_positioned(heap, src) {
        Ok(fs) => fs,
        // unparsable (e.g. mid-edit) — no diagnostics rather than an error
        Err(_) => return Ok(heap.list(Vec::new())),
    };
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = if here {
        crate::types::check::check_forms_here(heap, &just_forms)
    } else {
        crate::types::check::check_file(heap, &just_forms)
    };
    let line_kw = Value::keyword(value::intern("line"));
    let col_kw = Value::keyword(value::intern("col"));
    let msg_kw = Value::keyword(value::intern("message"));
    let mut out = Vec::with_capacity(warnings.len());
    for (pos_opt, msg) in &warnings {
        let msg_val = heap.alloc_string(msg);
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(3);
        if let Some(p) = pos_opt {
            entries.push((line_kw, Value::int(p.line as i64)));
            entries.push((col_kw, Value::int(p.col as i64)));
        }
        entries.push((msg_kw, msg_val));
        out.push(heap.map_from_pairs(entries));
    }
    Ok(heap.list(out))
}
