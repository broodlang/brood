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

// ---------- source positions (editor tooling; see docs/tooling.md) ----------

/// `(form-pos form)` — the `[line col]` (1-based) where `form` was read, or
/// `nil`. Recorded by the reader for list forms; used by the test macros to
/// capture a test's source line *before* the form expands.
pub(super) fn form_pos(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.form_pos_only(arg(args, 0)) {
        Some(p) => Ok(heap.alloc_vector(vec![Value::int(p.line as i64), Value::int(p.col as i64)])),
        None => Ok(Value::nil()),
    }
}

/// `(current-file)` — the path of the file currently being `load`ed, or `nil`
/// (e.g. at the REPL). Maintained by `load`.
/// `(source-location 'name)` — where `name`'s global definition was loaded from,
/// as `[file line col]`, or `nil` if it has no recorded site (a Rust builtin, or
/// an unknown/local name). Prelude globals resolve to a materialized copy of the
/// standard library. The site is captured at load time
/// before macroexpansion, so `defn`/`defmacro` definitions are located
/// accurately. The image-query foundation for cross-file goto-definition (ADR-031
/// / docs/lsp.md). Takes a symbol, so quote it: `(source-location 'foo)`.
pub(super) fn source_location(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "source-location",
                "symbol",
                other,
            ))
        }
    };
    match heap.def_site(name) {
        Some(loc) => {
            let file = heap.alloc_string(&loc.file);
            Ok(heap.alloc_vector(vec![
                file,
                Value::int(loc.pos.line as i64),
                Value::int(loc.pos.col as i64),
            ]))
        }
        None => Ok(Value::nil()),
    }
}

/// `(private? 'mod/name)` — whether the global `mod/name` is module-private
/// (ADR-146). Reads the recorded privacy fact (`Heap::is_private`), the single
/// source of truth every enforcement/import site now consults, rather than
/// re-deriving from the `--` marker in the name. Takes the qualified symbol as
/// given, so quote it: `(private? 'mod/helper--x)` → true, `(private? 'mod/pub)` →
/// false. An undefined or non-`--` name is not private.
pub(super) fn private_p(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Sym(s) => Ok(Value::boolean(heap.is_private(s))),
        other => Err(LispError::wrong_type(heap, "private?", "symbol", other)),
    }
}

/// `(type-signature 'name)` — the checker's type signature for the global `name`
/// (declared, curated, or inferred), as an arrow string like `"(int -> int)"`, or
/// `nil` when it has no pinnable signature (unbound, non-callable, or a value the
/// checker can't type). Reads the loaded image, same discipline as `arglist`/`doc`.
/// The introspection foundation the LSP hover and the `nest mcp` `lookup` tool
/// share (so they can't drift on what a name's type is). `name` may be a symbol or
/// a string; a never-interned name names no global, so it returns `nil`.
pub(super) fn type_signature(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            match value::intern_existing(&name) {
                Some(s) => s,
                None => return Ok(Value::nil()),
            }
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "type-signature",
                "symbol or string",
                other,
            ))
        }
    };
    match crate::types::check::signature_string(heap, sym) {
        Some(signature) => Ok(heap.alloc_string(&signature)),
        None => Ok(Value::nil()),
    }
}

/// `(references-in-source name source)` — every occurrence of the global `name`
/// in `source`, as a list of `[line col]` (both 1-based), in document order. A
/// local that shadows the name is excluded. Pure: it parses the string and
/// holds no project state, so the Brood-side `callers` MCP tool maps it over a
/// project's files for cross-file find-references (ADR-031 §Cross-file,
/// docs/lsp.md). `name` may be a symbol or a string.
pub(super) fn references_in_source(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "references-in-source",
                "symbol or string",
                other,
            ))
        }
    };
    let src = expect_string(heap, "references-in-source", arg(args, 1))?;
    let root = cst::parse(&src);
    let tree = crate::syntax::scope::analyze(&root, &src);
    let starts = line_starts(&src);
    let occ: Vec<Value> = tree
        .references_to_global(&root, &src, &name)
        .into_iter()
        .map(|span| {
            let (line, col) = line_col(&src, &starts, span.start as usize);
            heap.alloc_vector(vec![Value::int(line as i64), Value::int(col as i64)])
        })
        .collect();
    Ok(heap.list(occ))
}

/// Byte offsets of each line start in `src` (line 0 begins at 0). Built once so
/// repeated byte→line/col lookups in one source are cheap.
pub(super) fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// 1-based (line, col) of byte offset `b`, col counted in characters. `b` must
/// be a char boundary (CST spans always are).
pub(super) fn line_col(src: &str, starts: &[usize], b: usize) -> (u32, u32) {
    let line = starts.partition_point(|&s| s <= b) - 1; // 0-based
    let col = src[starts[line]..b].chars().count();
    (line as u32 + 1, col as u32 + 1)
}

pub(super) fn current_file(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.current_file().map(str::to_string) {
        Some(f) => Ok(heap.alloc_string(&f)),
        None => Ok(Value::nil()),
    }
}

// ---------- introspection (editor tooling; see docs/lsp.md) ----------

/// `(doc f)` — the docstring of a function or macro value, or `nil`. A docstring
/// is the leading string literal in a `fn`/`defn` body (stored on the closure
/// when more body follows it). Powers hover / `describe-function`.
pub(super) fn doc(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let text = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => heap.closure(id).doc.clone(),
        // A primitive's docstring lives on the `NativeFn` (the `PRIMITIVE_DOCS`
        // table), since it has no Brood body to carry a leading string.
        Value::Native(id) => {
            let d = heap.native(id).doc;
            (!d.is_empty()).then(|| d.to_string())
        }
        _ => None,
    };
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::nil()),
    }
}

/// `(arglist f)` — the parameter list of a function, macro, or primitive as a
/// list, mirroring the source surface: required names, then `&optional` names,
/// then `& rest`. `nil` for a non-function (or a primitive without recorded
/// params). Feeds signature help / hover.
pub(super) fn arglist(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => id,
        // A primitive carries its param names as a flat `&'static` list (incl. any
        // `&`/`&optional` markers, already in order) — hand them back as symbols.
        Value::Native(id) => {
            let params = heap.native(id).params;
            if params.is_empty() {
                return Ok(Value::nil());
            }
            let items: Vec<Value> = params.iter().map(|p| value::sym(p)).collect();
            return Ok(heap.list(items));
        }
        _ => return Ok(Value::nil()),
    };
    // Copy the parts out before re-borrowing the heap mutably to build the list.
    // For a multi-arity closure there's no single arglist; show the last clause
    // (conventionally the most general — e.g. the variadic `(a b & more)`).
    let (params, optionals, rest) = {
        let cl = heap.closure(id);
        let arm = cl.arms.last().expect("closure has at least one arm");
        (
            arm.params.clone(),
            arm.optionals.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
            arm.rest,
        )
    };
    let mut items: Vec<Value> = params.into_iter().map(Value::Sym).collect();
    if !optionals.is_empty() {
        items.push(value::sym("&optional"));
        items.extend(optionals.into_iter().map(Value::Sym));
    }
    if let Some(r) = rest {
        items.push(value::sym("&"));
        items.push(Value::symbol(r));
    }
    Ok(heap.list(items))
}

/// `(global-names)` — a list of every symbol bound in the global table
/// (prelude + user `def`s), sorted by spelling so the order is deterministic
/// (for completion / workspace-symbol tooling and reproducible doc generation).
/// Special forms and the core control/binding macros — the keyword-like heads:
/// the single source of truth for "what reads as a keyword". Read from Brood via
/// the `(special-forms)` primitive (so `std/editor/highlight.blsp` highlights from this
/// list) and from the LSP (`semantic_tokens` / `completion` import it rather than
/// keeping a copy), so the runtime and the tooling can't drift. Mirrors
/// `brood.el`'s `brood-special-forms` plus the `def`-family heads.
pub const SPECIAL_FORMS: &[&str] = &[
    kw::IF,
    kw::DO,
    kw::DEF,
    kw::FN,
    kw::LET,
    kw::LETREC,
    kw::QUOTE,
    kw::QUASIQUOTE,
    kw::DEFMACRO,
    kw::DEFN,
    kw::DEFDYN,
    kw::DEFRECORD,
    kw::DEFABILITY,
    kw::IMPL,
    kw::DEFMODULE,
    kw::WHEN,
    kw::UNLESS,
    kw::COND,
    kw::AND,
    kw::OR,
    kw::MATCH,
    kw::MATCH_STAR,
    kw::CASE,
    kw::COMMENT,
    kw::TRY,
    kw::CATCH,
    kw::THROW,
    kw::RECEIVE,
    kw::BINDING,
    kw::DOLIST,
    kw::DOSEQ,
    kw::DOTIMES,
    kw::FOR,
    kw::THREAD_FIRST,
    kw::THREAD_LAST,
    // Core macros (std/prelude.blsp) that read as keywords — highlight-only, not
    // evaluator special forms (ADR-092). Promoted here so every editor (VS Code via
    // `nest grammar`, Emacs, the REPL highlighter) + the LSP colour them from one
    // source. `throw`/`receive` are already above (they're in the core set).
    kw::SPAWN,
    kw::SPAWN_LINK,
    kw::REMOTE_SPAWN,
    kw::REMOTE_SPAWN_SYNC,
    kw::ERROR,
    kw::WITH_OUT_STR,
    kw::WITH_ERR_STR,
    kw::BENCH,
];

/// `(special-forms)` — the list of special-form / core-macro names (strings) that
/// read as keywords, for tooling (the highlighter, completion). Returns the
/// canonical `SPECIAL_FORMS`, so Brood and the LSP share one list.
pub(super) fn special_forms(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = SPECIAL_FORMS.iter().map(|s| heap.alloc_string(s)).collect();
    Ok(heap.list(items))
}

pub(super) fn global_names(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let mut syms = heap.global_symbols();
    // `symbol_name` locks the interner and allocates, so resolve each spelling
    // once (cached) rather than twice per comparison.
    syms.sort_by_cached_key(|&s| value::symbol_name(s));
    let syms: Vec<Value> = syms.into_iter().map(Value::Sym).collect();
    Ok(heap.list(syms))
}

/// `(bound? 'name)` — whether `name` is bound in the current scope (which
/// reaches the global table). Takes a symbol, so quote it: `(bound? 'foo)`.
pub(super) fn bound_p(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Sym(s) => Ok(Value::boolean(heap.env_get(env, s).is_some())),
        other => Err(LispError::wrong_type(heap, "bound?", "symbol", other)),
    }
}

pub(super) fn gensym(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(heap, other),
    };
    Ok(value::gensym(&prefix))
}
