//! The checker's test harness: the helpers every themed test file below shares —
//! `warnings` and friends build a full `Interp` (primitives + the loaded prelude) so the
//! unbound-symbol diagnostic sees every stdlib name — and the themed files themselves,
//! one per slice of the checker's behaviour.

use super::sigs::primitive_sig;
use super::*;
use crate::core::value::Tag;
use crate::syntax::reader;
use crate::types::Ty;

mod abilities;
mod closure_inference;
mod declarations;
mod declared_vs_curated;
mod discarded_catch_lint;
mod domains;
mod effective_signatures;
mod element_types;
mod gradual_checks;
mod inference_precision;
mod lints;
mod match_lints;
mod modules_and_imports;
mod names_as_types;
mod reach_gate;
mod refinement;
mod robustness;
mod scope_and_guards;
mod signatures;

/// A full `Interp` — primitives + the loaded prelude. We need the prelude
/// in the global env so the new unbound-symbol diagnostic doesn't false-
/// flag every Brood-side stdlib name (`list`, `int?`, `zero?`, `inc`, …);
/// the previous primitives-only setup worked when the checker silently
/// skipped unknown callees, but Step 4's unbound check has to know what's
/// genuinely bound.
fn warnings(src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    check_form(&interp.heap, form)
}

/// Build `(head (head … x))`, `depth` deep, straight on the heap (the reader caps
/// nesting far below this).
fn nest_form(interp: &mut crate::Interp, head: &str, depth: usize, mut form: Value) -> Value {
    let head = crate::core::value::intern(head);
    for _ in 0..depth {
        let tail = interp.heap.alloc_pair(form, Value::Nil);
        form = interp.heap.alloc_pair(Value::Sym(head), tail);
    }
    form
}

fn mk_list(interp: &mut crate::Interp, items: &[Value]) -> Value {
    let mut out = Value::Nil;
    for &item in items.iter().rev() {
        out = interp.heap.alloc_pair(item, out);
    }
    out
}

fn items_of(interp: &crate::Interp, v: Value) -> Vec<Value> {
    super::walk::list_items(&interp.heap, v).unwrap_or_default()
}

/// `warnings` but with macroexpansion — what `(check 'form)` and
/// `check-file` actually do. Required to exercise post-expansion shapes
/// like `match` (a `defmacro` whose pattern compiler lowers to
/// `let`+`if`+`%eq`), threading macros, and the test-framework wrappers.
/// Like [`warnings`], but with `mods` loaded first. A bare `Interp` carries the prelude
/// only, so a module's declared `sig`s are unknown and every cross-module check passes
/// vacuously — `brood --check` on a real file auto-requires, this harness does not.
fn warnings_with(mods: &[&str], src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    for m in mods {
        interp
            .eval_str(&format!("(require-one '{m})"))
            .unwrap_or_else(|e| panic!("require {m}: {e:?}"));
    }
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    check_form(&interp.heap, form)
}

fn warnings_expanded(src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    let form = crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
    check_form(&interp.heap, form)
}

/// Whole-file checking — what `nest check` runs. Unlike [`warnings`] (a bare
/// fragment), this enables operand / value-slot unbound checking and threads
/// file-local def names, so it exercises the strict, file-mode behaviour.
fn file_warnings(src: &str) -> Vec<String> {
    file_warnings_mode(src, false)
}

fn file_warnings_mode(src: &str, strict: bool) -> Vec<String> {
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    check_file_mode(&mut heap, &forms, &[], strict)
        .into_iter()
        .map(|(_, m)| m)
        .collect()
}

/// The checker's type of one expression, rendered — for pinning the answers the
/// precision rules give. Wrapped in `(list …)` so the position-keyed query has a call to
/// anchor on; `arg_ty_at` then types item 1, the expression itself.
fn ty_str(src: &str) -> String {
    arg_ty_of(&format!("(list {src})"), "(list", 1)
        .map(|t| t.to_string())
        .unwrap_or_else(|| "<unknown>".into())
}

/// The non-tail-recursion lint (`recursion::check_recursion`) over a
/// macroexpanded form — what `check-file`'s Pass 3.5 runs.
fn recursion_warnings(src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    let form = crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
    let mut out = Vec::new();
    recursion::check_recursion(&interp.heap, form, &mut out);
    out.into_iter().map(|(_, m)| m).collect()
}

// ------------- Step 3: sigs sourced from NativeFn, closure inference --------------

/// The eight test cases below need real user-defined closures, which means
/// running a `defn` against the global table. The `Interp` builds the full
/// prelude (curated stdlib closures and all) on top of the primitive kernel
/// — exactly the surface a checker is supposed to see.
fn check_with_defs(defs: &[&str], src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    for d in defs {
        interp.eval_str(d).expect("def");
    }
    let form = crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse expression");
    // Macro-expand so any prelude wrappers (defn → fn, etc.) are gone, like
    // `brood --check`/the `check` builtin do before calling check_form.
    let form = crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
    check_form(&interp.heap, form)
}

/// `arg_ty_at` — the position-keyed type query behind the LSP record-field
/// completion. Shared harness: parse `src` positioned, find the line/col of
/// `needle`'s opening paren, and ask for the type of the call's item 1.
fn arg_ty_of(src: &str, needle: &str, arg_index: usize) -> Option<Ty> {
    let mut interp = crate::Interp::new();
    let positioned = reader::read_all_positioned(&mut interp.heap, src).expect("parse");
    let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
    let at = src.find(needle).expect("needle present");
    let line = src[..at].bytes().filter(|&b| b == b'\n').count() as u32 + 1;
    let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = src[line_start..at].chars().count() as u32 + 1;
    arg_ty_at(&mut interp.heap, &forms, line, col, arg_index)
}

fn field_names(ty: &Ty) -> Vec<String> {
    ty.record_fields()
        .map(|f| f.keys().map(|&s| value::symbol_name(s)).collect())
        .unwrap_or_default()
}

fn planted_name(src: &str) -> String {
    let start = src
        .find("zzz-")
        .expect("every reach case plants a `zzz-…` name");
    let rest = &src[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

// ---- effective signatures for a buffer (the LSP inlay-hint source) ----

/// `file_signatures` over a source string, as `name → rendered sig (declared?)`.
fn signatures(src: &str) -> Vec<(String, String, bool)> {
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    super::file_signatures(&mut heap, &forms)
        .into_iter()
        .map(|s| (s.name, s.sig.to_string(), s.declared))
        .collect()
}

// ---- declared-vs-curated precision (2026-08-28) -------------------------------------

/// Every `.blsp` under `dir`, recursively.
fn blsp_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            blsp_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "blsp") {
            out.push(p);
        }
    }
}

/// The `(defmodule NAME …)` name, read textually so this gate does not depend on the
/// module-form API it is checking around. `None` for a bare-rooted (prelude) file.
fn declared_module_name(src: &str) -> Option<String> {
    let rest = src.split_once("(defmodule")?.1;
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '(' && *c != ')')
        .collect();
    (!name.is_empty()).then_some(name)
}
