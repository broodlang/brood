// Extracted from system.rs (file-organization split).
#![allow(unused_imports)]
use super::numeric::{arg, expect_int, expect_string, expect_symbol};
use super::system::*;
use super::*;
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::compile::apply_engine;
use crate::syntax::{cst, printer, reader};

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
    let signatures = crate::types::check::file_signatures(heap, &just_forms);
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
    Ok(heap.list(out))
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

/// `(check-string-structured src)` — the source-string counterpart of
/// `check-file-structured`: advisory type-check the Brood source string `src` and
/// return a list of `{:line :col :message}` maps (1-based positions; no `:file`).
/// Returns `()` when `src` doesn't parse — e.g. incomplete input while an editor
/// buffer is mid-edit — so a live diagnostics loop never errors on an unbalanced
/// buffer; warnings reappear once it parses. Reuses the same checker as the file
/// variant (`types::check::check_file`).
pub(super) fn check_string_structured(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "check-string-structured", arg(args, 0))?;
    let forms = match reader::read_all_positioned(heap, &src) {
        Ok(fs) => fs,
        // unparsable (e.g. mid-edit) — no diagnostics rather than an error
        Err(_) => return Ok(heap.list(Vec::new())),
    };
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
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
