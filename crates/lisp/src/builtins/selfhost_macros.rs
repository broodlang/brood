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
/// by `(check-project)` in `std/tool/project.blsp` for the `nest test` / `nest run`
/// pre-flight.
pub(super) fn check_file_builtin(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("check-file: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
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
    let (warnings, dep_keys) = crate::types::check::check_file_with_deps(heap, &just_forms);
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
    let warnings = crate::types::check::check_file(heap, &just_forms);
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
