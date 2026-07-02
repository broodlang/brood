use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::{cst, printer, reader};

use super::numeric::{arg, expect_int, expect_string, expect_symbol};
use super::realize_seqview;
use crate::core::keywords as kw;
use crate::eval::compile::apply_engine;
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

// ---------- self-hosting ----------

pub(super) fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    crate::eval::eval(heap, form, root)
}

pub(super) fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-string", arg(args, 0))?;
    reader::read_one_complete(heap, &s)
}

/// `(read-first s)` — parse and return the **first** form in `s`, ignoring any
/// trailing forms. The lenient sibling of `read-string`: for peeking the leading
/// form of a multi-form source (e.g. a file's `(defmodule …)` header) without
/// parsing — or erroring on — the rest.
pub(super) fn read_first(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-first", arg(args, 0))?;
    reader::read_one(heap, &s)
}

/// `(read-all s)` — parse *every* form in `s` and return them as a list (empty for
/// blank/comment-only input). The all-forms sibling of `read-string` (which
/// returns only the first), and the read-half of `eval-string` without the eval —
/// so form-manipulating Brood (an editor evaluating the last sexp before point,
/// say) can isolate individual forms. Raises on a malformed/incomplete form, like
/// `read-string`; use `parse-source` for lossless, error-tolerant parsing.
pub(super) fn read_all(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-all", arg(args, 0))?;
    let forms = reader::read_all(heap, &s)?;
    Ok(heap.list(forms))
}

/// `(parse-source s)` — parse s into a lossless CST tree as nested vectors, the
/// mechanism behind `std/format.blsp`. Never raises: malformed input becomes
/// `[:error "raw"]` nodes (parsing resumes after them). See `syntax::cst`.
///
/// Shape (each node is a vector `[kind …]`):
/// - Leaves carry the original source text:
///   `[:symbol "foo"]`, `[:keyword ":foo"]`, `[:int "42"]`, `[:float "1.5"]`,
///   `[:bool "true"]`, `[:nil "nil"]`, `[:str "\"hi\""]` (raw — quotes/escapes
///   included), `[:whitespace "  \n"]`, `[:comment ";; hi\n"]`, `[:error "raw"]`.
/// - Reader macros wrap a single child form:
///   `[:quote child]`, `[:quasi child]`, `[:unquote child]`, `[:splice child]`.
/// - Containers carry a child vector:
///   `[:root [child …]]`, `[:list [child …]]`, `[:vector [child …]]`,
///   `[:map [child …]]`.
///
/// Roundtrip property: concatenating every leaf's text in tree order reproduces
/// the input — this is what makes the CST a faithful basis for formatting.
pub(super) fn parse_source(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "parse-source", arg(args, 0))?;
    let root = cst::parse(&s);
    Ok(cst_to_value(heap, &root, &s))
}

pub(super) fn cst_to_value(heap: &mut Heap, node: &cst::Node, src: &str) -> Value {
    use cst::NodeKind::*;
    let tag = |k: &'static str| Value::keyword(value::intern(k));
    match node.kind {
        // Leaves: [kind raw-text].
        Symbol | Keyword | Int | Float | Decimal | Str | Bool | Nil | Whitespace | Comment
        | Error => {
            let k = match node.kind {
                Symbol => "symbol",
                Keyword => "keyword",
                Int => "int",
                Float => "float",
                Decimal => "decimal",
                Str => "str",
                Bool => "bool",
                Nil => "nil",
                Whitespace => "whitespace",
                Comment => "comment",
                Error => "error",
                _ => unreachable!(),
            };
            let text = heap.alloc_string(node.text(src));
            heap.alloc_vector(vec![tag(k), text])
        }
        // Reader-macro wrappers: [kind child]. The single structural child is
        // the wrapped form; any leading whitespace child is dropped (the wrapper
        // owns its position via its parent's children list).
        Quote | Quasi | Unquote | Splice => {
            let k = match node.kind {
                Quote => "quote",
                Quasi => "quasi",
                Unquote => "unquote",
                Splice => "splice",
                _ => unreachable!(),
            };
            // A reader-macro node's children are the wrapped form's parse
            // result(s) — usually a single form. Walk and pick the first
            // non-trivia child; nest the rest as following siblings would be a
            // parse bug, but in case of empty (EOF after ~/`/'/), emit nil.
            let child = node
                .forms()
                .next()
                .map(|c| cst_to_value(heap, c, src))
                .unwrap_or(Value::nil());
            heap.alloc_vector(vec![tag(k), child])
        }
        // Containers: [kind [child …]]. Children include trivia (whitespace +
        // comments) so the formatter can preserve blank-line + comment intent.
        Root | List | Vector | Map => {
            let k = match node.kind {
                Root => "root",
                List => "list",
                Vector => "vector",
                Map => "map",
                _ => unreachable!(),
            };
            let kids: Vec<Value> = node
                .children
                .iter()
                .map(|c| cst_to_value(heap, c, src))
                .collect();
            let kids_vec = heap.alloc_vector(kids);
            heap.alloc_vector(vec![tag(k), kids_vec])
        }
    }
}

/// `(parse-source-positioned s)` — like `parse-source`, but every CST node is a
/// MAP carrying its absolute source position rather than a `[kind …]` vector:
/// `{:kind :start :end}` for leaves (plus `:text`, the leaf's raw source), and
/// additionally `:kids` (a vector of child node maps) for containers
/// (`:root`/`:list`/`:vector`/`:map`) and reader-macro wrappers
/// (`:quote`/`:quasi`/`:unquote`/`:splice`). `:start`/`:end` are half-open
/// CHARACTER offsets (not bytes) — matching `string-length` and editor buffer
/// point — so structural tooling (`std/sexp`) navigates the tree directly.
///
/// The kernel already tracks every node's span; this projects it in one pass. It
/// exists because recovering those positions in interpreted Brood (`std/sexp`'s
/// former `annotate` walk) was O(n) and dominated structural-navigation latency.
pub(super) fn parse_source_positioned(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "parse-source-positioned", arg(args, 0))?;
    let root = cst::parse(&s);
    let b2c = byte_to_char_offsets(&s);
    Ok(cst_to_positioned(heap, &root, &s, &b2c))
}

/// Per-byte → character-offset table for `s`: `t[b]` is the count of characters
/// before byte offset `b`. Length `s.len() + 1` so a node's `span.end` (which can
/// equal `s.len()`) is indexable. CST spans land on char boundaries; a byte
/// interior to a multi-byte char maps to that char's own index (never queried).
pub(super) fn byte_to_char_offsets(s: &str) -> Vec<u32> {
    let mut t = vec![0u32; s.len() + 1];
    let mut byte = 0usize;
    let mut ci = 0u32;
    for ch in s.chars() {
        let w = ch.len_utf8();
        for k in 0..w {
            t[byte + k] = ci;
        }
        byte += w;
        ci += 1;
    }
    t[s.len()] = ci;
    t
}

pub(super) fn cst_node_kind_name(kind: cst::NodeKind) -> &'static str {
    use cst::NodeKind::*;
    match kind {
        Symbol => "symbol",
        Keyword => "keyword",
        Int => "int",
        Float => "float",
        Decimal => "decimal",
        Str => "str",
        Bool => "bool",
        Nil => "nil",
        Whitespace => "whitespace",
        Comment => "comment",
        Error => "error",
        Quote => "quote",
        Quasi => "quasi",
        Unquote => "unquote",
        Splice => "splice",
        Root => "root",
        List => "list",
        Vector => "vector",
        Map => "map",
    }
}

pub(super) fn cst_to_positioned(
    heap: &mut Heap,
    node: &cst::Node,
    src: &str,
    b2c: &[u32],
) -> Value {
    use cst::NodeKind::*;
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    let start = Value::int(b2c[node.span.start as usize] as i64);
    let end = Value::int(b2c[node.span.end as usize] as i64);
    let mut pairs: Vec<(Value, Value)> = vec![
        (kw("kind"), kw(cst_node_kind_name(node.kind))),
        (kw("start"), start),
        (kw("end"), end),
    ];
    match node.kind {
        // Leaves carry their raw source text; positions alone make them navigable.
        Symbol | Keyword | Int | Float | Decimal | Str | Bool | Nil | Whitespace | Comment
        | Error => {
            let text = heap.alloc_string(node.text(src));
            pairs.push((kw("text"), text));
        }
        // Containers + wrappers carry their (position-annotated) children — trivia
        // included, exactly as `parse-source`, so callers filter what they want.
        Quote | Quasi | Unquote | Splice | Root | List | Vector | Map => {
            let kids: Vec<Value> = node
                .children
                .iter()
                .map(|c| cst_to_positioned(heap, c, src, b2c))
                .collect();
            let kids_vec = heap.alloc_vector(kids);
            pairs.push((kw("kids"), kids_vec));
        }
    }
    heap.map_from_pairs(pairs)
}

/// `(tree-sitter-parse source lang)` — parse a foreign language into the same
/// positioned-CST node shape as `parse-source-positioned`. Mechanism lives in
/// `crate::treesit` (feature-gated); this just unwraps the args. See §C.
pub(super) fn tree_sitter_parse(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "tree-sitter-parse", arg(args, 0))?;
    let lang = match arg(args, 1) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "tree-sitter-parse",
                "keyword",
                v,
            ))
        }
    };
    crate::treesit::parse(heap, &src, &lang)
}

/// `(tree-sitter-reparse key source lang)` — incremental re-parse keyed by buffer
/// id `key`; same positioned CST as `tree-sitter-parse`, less work. Mechanism in
/// `crate::treesit` (feature-gated).
pub(super) fn tree_sitter_reparse(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let key = expect_int(heap, "tree-sitter-reparse", arg(args, 0))?;
    let src = expect_string(heap, "tree-sitter-reparse", arg(args, 1))?;
    let lang = match arg(args, 2) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "tree-sitter-reparse",
                "keyword",
                v,
            ))
        }
    };
    crate::treesit::parse_incremental(heap, key, &src, &lang)
}

/// `(tree-sitter-forget key)` — drop the cached incremental tree(s) for buffer
/// `key`; returns the count dropped. Call when a buffer closes.
pub(super) fn tree_sitter_forget(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let key = expect_int(heap, "tree-sitter-forget", arg(args, 0))?;
    Ok(Value::int(crate::treesit::forget(key)))
}

/// `(reload-defs path)` — like `load`, but only re-evaluates **definitions**
/// (`def`/`defmacro` and `def…`-named macros: `defn`, `defmodule`, `defdyn`,
/// `defonce`, user definers). All other top-level forms — `(require …)`,
/// `(load …)`, a `(main-loop 0)` entry call — are silently skipped. Used by the
/// file watcher (`std/reload.blsp`): on the **second** and subsequent visits to
/// a file we want to refresh the code (so the running program sees the new
/// behaviour via late binding) but **not** re-run side-effecting top-level calls
/// — re-executing those would spawn a duplicate long-running process (a
/// tail-recursive loop) or block the watcher itself.
///
/// **Atomicity:** the whole file is read before any form is evaluated, so a
/// half-saved / syntactically broken file applies *zero* defs (read fails
/// first). Forms are then expanded+evaluated one at a time, exactly like
/// `load`, so a macro a form defines is visible to later forms in the same file
/// (`lib.rs`). The residual non-atomic window is a *runtime* error while
/// evaluating form N, after 1..N-1 already landed; full snapshot/rollback is
/// deferred (docs/live-editing.md Stage 2). Returns `nil`. ADR-013 hot reload's
/// mechanism flowing through to the tool layer.
pub(super) fn reload_defs(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "reload-defs", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("reload-defs: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    let prev = heap.set_current_file(Some(path.clone()));
    // Namespace bracketing + forward-ref pre-scan, like `load` (ADR-065): a
    // reloaded namespaced file re-establishes its own namespace (its `(ns …)` form
    // is re-evaluated below) so its re-saved defs are qualified correctly.
    let prev_ns = heap.set_compile_ns(None);
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let known = if crate::eval::macros::file_opens_ns(heap, &form_vals) {
        crate::eval::macros::scan_def_names(heap, &form_vals)
    } else {
        std::collections::HashSet::new()
    };
    let prev_known = heap.set_ns_known_names(known);
    let prev_imports = heap.set_imports(std::collections::HashMap::new());
    let mut result = Ok(Value::nil());
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds; re-fetch
    // each from the (relocated) root stack rather than the stale `forms` Vec. Same
    // discipline as `load`.
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        // Re-eval only *definitions*; skip side-effecting top-level forms
        // (`(require …)`, `(load …)`, a `(main-loop 0)` entry call). A form is a
        // definition when its head symbol starts with "def" **and** is actually a
        // definer — one of the `def`/`defmacro` core special forms, or a symbol
        // currently bound to a macro (`defn`/`defmodule`/`defdyn`/`defonce` and
        // any user `def…` macro). The macro check drops the false positive on a
        // plain top-level *call* to a function whose name merely starts with
        // "def" (e.g. `(default-config)`): that head resolves to a `Fn`, not a
        // macro, so it's correctly skipped.
        //
        // Known limitation (accepted — docs/live-editing.md Stage 2): a definer
        // macro *not* named `def…` (e.g. `(register-handler …)` expanding to a
        // `def`) is skipped. Workaround: prefix definer macros with `def`, the
        // Lisp convention anyway. (`require` skipping is likewise intentional: we
        // don't transitively reload other modules; the user watches each path
        // explicitly with `reload-on-change`.)
        let head_is_def = match form {
            Value::Pair(p) => {
                let (head, _) = heap.pair(p);
                match head {
                    Value::Sym(s) => {
                        let nm = value::symbol_name(s);
                        // The `(defmodule …)` header is re-evaluated too (so the
                        // reloaded file's namespace + imports are re-established for
                        // its defs, ADR-065) — it's a `def…`-named macro, caught here.
                        //
                        // Resolve the head through the current namespace + imports
                        // before the macro check, so a *module-qualified* definer
                        // macro used bare (e.g. `deflive` from `(:use web/live)`,
                        // bound as `web/live/deflive`, not in root) is still
                        // recognised and re-evaluated. Without this, a `(deflive …)`
                        // top-level form would be skipped and its defs never reload.
                        nm.starts_with("def")
                            && (nm == "def" || nm == "defmacro" || {
                                let resolved = crate::eval::macros::resolve_reference(heap, s);
                                matches!(heap.env_get(root, resolved), Some(Value::Macro(_)))
                            })
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !head_is_def {
            continue;
        }
        // Same def-site recording / expand / eval shape as `load` for the
        // forms we *do* evaluate, so cross-file goto still lands at the
        // re-saved def site.
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    result.map(|_| Value::Nil)
}

pub(super) fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "load", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("load: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    // Read positioned so errors point at a line; tag every error with the file
    // (`FILE:LINE:COL:`, see docs/tooling.md).
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    // Expose the file to Brood (`(current-file)`) for the duration of the load,
    // so the test macros can record each test's source location; restore the
    // previous file afterward since loads nest.
    let prev = heap.set_current_file(Some(path.clone()));
    // A loaded file starts at the root namespace and its own `(ns …)` form sets the
    // current namespace for the rest of the file (ADR-065); restore the caller's
    // namespace afterward so loads nest and ns state never leaks out of a file.
    let prev_ns = heap.set_compile_ns(None);
    // Forward-reference pre-scan (ADR-065): if the file opens a namespace, record
    // the bare names it will define so a reference to a later definition resolves.
    // Cheap (read-only, no GC), gated on the file actually using `(ns …)`.
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let known = if crate::eval::macros::file_opens_ns(heap, &form_vals) {
        crate::eval::macros::scan_def_names(heap, &form_vals)
    } else {
        std::collections::HashSet::new()
    };
    let prev_known = heap.set_ns_known_names(known);
    let prev_imports = heap.set_imports(std::collections::HashMap::new());

    // **Bounded loading — the core memory guarantee (docs/memory-review.md).**
    // The collector now reclaims at ANY eval depth (ADR-061), so a file loaded
    // here is bounded no matter how deep `(load …)` sits — no `GcBlockReset`
    // depth-1 trick is needed any more. We still root the unevaluated forms across
    // the per-form eval: a collection during form `i` relocates the LOCAL forms
    // `i+1..` this loop still holds, so we re-fetch each from the (relocated) root
    // stack via `root_at` rather than the stale `forms` Vec. (Living in `load`,
    // the core, means every entry path — `brood`, `nest`, MCP `eval`, the future
    // editor — inherits the bound for free.)
    let mut result = Ok(Value::nil());
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    result
}

/// `(eval-string "src")` — read and evaluate every form in a string against the
/// global environment. Inherits the current namespace (ADR-065): the REPL evaluates
/// each entry through here, so a `(ns foo)` typed at the REPL sticks to later
/// entries. To load a *module* source at the root namespace, use `%load-string`.
pub(super) fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "eval-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, false)
}

/// `(%load-string "src")` — the string analogue of `load`: read+eval every form,
/// but bracket the current namespace (reset to root, restore the caller's after),
/// so an embedded module's own `(ns …)` governs it and ns state doesn't leak to the
/// caller. Used by `require-one` for baked-in std modules (ADR-065).
pub(super) fn load_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, true)
}

/// Shared body of `eval-string` / `%load-string`. When `reset_ns`, the current
/// namespace is reset to root for the duration and the caller's restored after.
pub(super) fn eval_string_inner(
    heap: &mut Heap,
    env: EnvId,
    src: &str,
    reset_ns: bool,
) -> LispResult {
    let root = heap.env_root(env);
    let forms = reader::read_all(heap, src)?;
    // When loading a module (`reset_ns`), bracket the namespace at root and
    // pre-scan its def heads for forward references; the plain `eval-string` (REPL,
    // inline) inherits the current namespace and does neither (ADR-065).
    let (prev_ns, prev_known, prev_imports) = if reset_ns {
        let pn = heap.set_compile_ns(None);
        let known = if crate::eval::macros::file_opens_ns(heap, &forms) {
            crate::eval::macros::scan_def_names(heap, &forms)
        } else {
            std::collections::HashSet::new()
        };
        let pk = heap.set_ns_known_names(known);
        let pi = heap.set_imports(std::collections::HashMap::new());
        (Some(pn), Some(pk), Some(pi))
    } else {
        (None, None, None)
    };
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds.
    let base = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let mut result: LispResult = Ok(Value::nil());
    for i in 0..forms.len() {
        let form = heap.root_at(base + i);
        match crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
        {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(base);
    if let Some(pn) = prev_ns {
        heap.set_compile_ns(pn);
    }
    if let Some(pk) = prev_known {
        heap.set_ns_known_names(pk);
    }
    if let Some(pi) = prev_imports {
        heap.set_imports(pi);
    }
    result
}

/// Standard-library modules baked into the binary (like the prelude), so they load
/// from any directory with no file paths. The require / provide / load-path
/// *policy* is written in Brood (`std/prelude.blsp`, ADR-019); Rust only exposes
/// an embedded module's source here, via `%builtin-module` (ADR-006/008).
///
/// Split into [`CORE_MODULES`] (always baked in) and [`DEV_MODULES`] (only under
/// the `dev-tools` feature), so a `nest release` lean runtime
/// (`--no-default-features`) carries no test/observer/tooling/REPL source
/// (ADR-038, docs/release.md). `builtin_module` consults both.
const CORE_MODULES: &[(&str, &str)] = &[
    // Output ports: the redirectable sink behind print/println — a port is a 1-arg
    // string sink, with `process-port`/`fn-port` + `with-out`/`with-err`. Pairs
    // with the prelude's `*out*`/`*err*` dynamic vars. Opt-in, no dependencies.
    ("io", include_str!("../../../../std/io.blsp")),
    // Fuzzy (subsequence) string matching + ranking: `fuzzy-match` / `fuzzy-filter`,
    // the matcher completion UIs ride on. Pure Brood, no dependencies. Opt-in.
    ("fuzzy", include_str!("../../../../std/fuzzy.blsp")),
    // Plain-text utilities (pure string->string): `fill` greedy word-wraps to a column
    // width — the engine behind an editor's fill-paragraph / M-q, and reusable for
    // wrapping help text or terminal output. No dependencies. Opt-in.
    ("text", include_str!("../../../../std/text.blsp")),
    ("project", include_str!("../../../../std/tool/project.blsp")),
    // The package manager (ADR-037): resolves the manifest's :dependencies into a
    // lock file + load-path entries. Required lazily by `project-setup` only when a
    // project actually declares deps. Opt-in, never in the prelude.
    ("package", include_str!("../../../../std/tool/package.blsp")),
    // TCP sockets (ADR-062): active-socket helpers + a spawn-per-connection
    // server over the non-blocking tcp-* primitives. Opt-in, never in the prelude.
    ("net/tcp", include_str!("../../../../std/net/tcp.blsp")),
    // The file & filesystem library: whole-file/line I/O, directory walking, path
    // helpers — Brood over the fs primitives. Opt-in, never in the prelude.
    ("file", include_str!("../../../../std/file.blsp")),
    // A minimal HTTP/1.0 server (ADR-062) over the tcp + file libraries — request
    // parsing, response rendering, a router, static files. Opt-in.
    ("net/http", include_str!("../../../../std/net/http.blsp")),
    // JSON ↔ Brood data, written entirely in Brood (a recursive-descent parser +
    // encoder over the string primitives; the reader's `\u{}` escape is the
    // codepoint→char mechanism). Opt-in, never in the prelude.
    ("json", include_str!("../../../../std/json.blsp")),
    // Server-Sent Events (text/event-stream): a client reader process that streams
    // events to a subscriber's mailbox (pairs with ui's `with-events`) + server-side
    // framing. Pure frame parsing + a thin IO loop over tcp; reuses http's URL/header
    // helpers. Opt-in.
    ("net/sse", include_str!("../../../../std/net/sse.blsp")),
    // The process framework, bundled in the default install (ADR-085 amended —
    // batteries-included, not externalized). `proc/gen` is the gen_server-style
    // server loop (`defprocess` / `spawn-server` / `!` / `gen-call` / `stop`); the
    // core `log` module is a `proc/gen` process. `proc/supervisor` is OTP-style
    // supervision — independent of `proc/gen`, both over the same kernel primitives.
    ("proc/gen", include_str!("../../../../std/proc/gen.blsp")),
    (
        "proc/supervisor",
        include_str!("../../../../std/proc/supervisor.blsp"),
    ),
    // Order a flat process-info snapshot as a parent→child forest (depth-tagged, DFS
    // by id). A pure, dependency-free transform — CORE, not dev-tools: it's shared by
    // the dev observer's tree sort *and* a shipped app's process list (myedit's
    // *Process List*), so a `nest release` binary needs it baked in.
    (
        "proctree",
        include_str!("../../../../std/tool/proctree.blsp"),
    ),
    // Run a thunk off the current process with an optional timeout + cancel
    // (ADR-006): `task` (async, tagged-reply handle), `cancel-task`, and the
    // synchronous `await`. Pure Brood over spawn / receive / exit — the generic
    // version of the editor's hand-rolled async-eval watchdog. Opt-in.
    ("task", include_str!("../../../../std/task.blsp")),
    // An async, safe logger (ADR-006): a `proc/gen` process holding a list of
    // backends, each an `io` port + a min level + a formatter. Log calls are casts
    // (fire-and-forget = async); the one process serialises writes (no interleaving)
    // and isolates a backend crash. Opt-in, never in the prelude.
    ("log", include_str!("../../../../std/log.blsp")),
    // Erlang :telemetry-style instrumentation (ADR-106). Handlers run in a dedicated
    // LISTENER process (emit is a fire-and-forget send), so a buggy handler can never
    // crash/hang the emitting process — only the listener, which a throwing handler
    // doesn't even do (caught + detached). The handler table is a `def`-rebound global
    // that survives a listener restart (ADR-013). `span` brackets a body with
    // :start/:stop/:exception events; `forward` runs handler work in your own process.
    // Opt-in, never in the prelude.
    ("telemetry", include_str!("../../../../std/telemetry.blsp")),
    // Date and time utilities (UTC): epoch↔datetime conversion, ISO 8601
    // format/parse, arithmetic, calendar predicates. Pure Brood over `now`.
    ("datetime", include_str!("../../../../std/datetime.blsp")),
    // Hex and Base64 encoding/decoding. Pure Brood over `char->int` /
    // `string->utf8-bytes` / `utf8-bytes->string`. Opt-in, never in the prelude.
    ("encoding", include_str!("../../../../std/encoding.blsp")),
    // Descriptive statistics over numeric sequences: mean, median, stddev,
    // variance, percentile, mode, frequencies. Pure Brood over sort/fold/sqrt.
    ("stats", include_str!("../../../../std/stats.blsp")),
    // Pull-stream protocol + combinators over green processes. Sources: list,
    // fn-generator, range, TCP socket. Transformers: map/filter/take/drop/
    // take-while/chunk/concat/lines. Terminals: fold/to-list/to-vector/
    // for-each/pipe/to-socket. Foundation for the HTTP streaming layer.
    ("stream", include_str!("../../../../std/stream.blsp")),
    // URL encoding/decoding and parsing: percent-encode/decode, query-string
    // encode/decode, parse-url, build-url. Pure Brood over string primitives.
    ("url", include_str!("../../../../std/url.blsp")),
    // CSV parsing and emitting: csv-parse, csv-parse-maps, csv-emit,
    // csv-emit-maps. Handles quoted fields, escaped quotes, \r\n endings.
    ("csv", include_str!("../../../../std/csv.blsp")),
    // RFC 4122 version-4 UUID generation via the OS CSPRNG (random-token).
    // uuid-v4, uuid-nil, uuid?.
    ("uuid", include_str!("../../../../std/uuid.blsp")),
    // {{var}} string templating: render a template string against a data map.
    // render, render-all.
    ("template", include_str!("../../../../std/template.blsp")),
    // Purely functional FIFO queue (two-list, amortised O(1)) and min-priority
    // queue (sorted-list, O(n) insert / O(1) pop).
    ("queue", include_str!("../../../../std/queue.blsp")),
    // Multi-valued map: one key may hold multiple values (a map of lists).
    // multimap-assoc, multimap-get, multimap-get-all, multimap-dissoc, …
    ("multimap", include_str!("../../../../std/multimap.blsp")),
    // MD5/SHA-1/SHA-256/SHA-384/SHA-512 + HMAC, all Brood over the two `%digest`
    // / `%hmac` prims (raw bytes); hex/string shaping via bytes->hex; hash-string is djb2.
    ("hash", include_str!("../../../../std/hash.blsp")),
    // LCS-based sequence diff: diff-seq, diff-lines, diff-summary, diff-patch,
    // diff-unified. O(m*n) time/space; suitable for small-to-medium sequences.
    ("diff", include_str!("../../../../std/diff.blsp")),
    // Path string manipulation: join, split, basename, dirname, extension, stem,
    // normalize, relative-to. Consolidates the prelude's path-* globals under
    // a single path/ namespace with additional operations.
    ("path", include_str!("../../../../std/path.blsp")),
    // OS/process interface: env vars, argv, subprocess execution, OS type, halt.
    // Wraps the %env-all/%argv/%os-cmd/%os-type/%halt primitives with a clean API.
    ("system", include_str!("../../../../std/system.blsp")),
    // Authenticated encryption (ChaCha20-Poly1305), PBKDF2 key derivation, secure
    // random bytes. Wraps the %chacha20-* and %pbkdf2-sha256-bytes primitives.
    ("crypto", include_str!("../../../../std/crypto.blsp")),
    // Process-backed state cell: start/get/update/get-and-update/cast/stop.
    // A thin Brood layer over spawn/send/receive for the common "stateful process" case.
    ("agent", include_str!("../../../../std/agent.blsp")),
    // The editor framework's buffer model (M2 Phase 1, ADR-045): an immutable
    // buffer over the rope primitives, opt-in, never in the prelude.
    (
        "editor/buffer",
        include_str!("../../../../std/editor/buffer.blsp"),
    ),
    // The display/input seam (M3, ADR-046): `display` is the render-op protocol
    // (pure data constructors); `keymap` is the rebindable key→command dispatcher
    // shared by the line editor and the observer; `observer` is a process-viewer
    // built on them + the `term-*`/`gui-*` primitives. All opt-in, never in the prelude.
    // The shared named-face / theme registry (the counterpart to `keymap`): style
    // named once, referenced everywhere, restyled in one place. Required by `ui`
    // (so every ui-run app gets it) and the observer.
    (
        "editor/face",
        include_str!("../../../../std/editor/face.blsp"),
    ),
    (
        "editor/display",
        include_str!("../../../../std/editor/display.blsp"),
    ),
    (
        "editor/keymap",
        include_str!("../../../../std/editor/keymap.blsp"),
    ),
    // Composable, runtime-reconfigurable behaviour layers over `keymap` (the
    // generic mechanism the editor's "modes" are built from; buffer-agnostic).
    // Opt-in, never in the prelude. See docs/layers.md.
    (
        "editor/layers",
        include_str!("../../../../std/editor/layers.blsp"),
    ),
    // Structural (s-expression) navigation over the parse-source CST — reusable
    // Brood-code tooling (same tier as the formatter / LSP), not editor-specific.
    // (The text-mode/brood-mode *layers* built on it are editor policy and live in
    // the editor app — examples/editor/src/ — not here.) Opt-in. (docs/layers.md)
    ("sexp", include_str!("../../../../std/tool/sexp.blsp")),
    // A small backtracking regular-expression engine, pure Brood (literals, ., * + ?,
    // ^ $, [...] sets, \d \w \s, |, groups; no ranges/captures yet). Opt-in.
    ("regex", include_str!("../../../../std/regex.blsp")),
    // ANSI / VT100 escape-sequence stripping for pipe output (CSI sequences + CR).
    // Used by bshell and compile to clean subprocess output before display.
    ("ansi", include_str!("../../../../std/ansi.blsp")),
    ("editor/ui", include_str!("../../../../std/editor/ui.blsp")),
    // Serve a `ui-run` app to remote frontends — the Emacs `--daemon`/`emacsclient`
    // model (ADR-090): the app runs on the daemon, a thin `attach` client paints
    // pushed frames + ships back keys. Pure Brood over `ui-run` + the node link.
    (
        "editor/serve",
        include_str!("../../../../std/editor/serve.blsp"),
    ),
    // Emacs-style tiled window splits: an immutable binary layout tree + pure
    // pane/divider geometry + drag-to-resize over `:drag` mouse events (ADR-077).
    // Reusable editor toolkit (content-agnostic); the keybindings + payload are
    // editor policy. Opt-in, never in the prelude.
    (
        "editor/pane",
        include_str!("../../../../std/editor/pane.blsp"),
    ),
    // Bare ANSI escape *strings* for simple terminal scripts (`print` them
    // directly) — the lightweight counterpart to the `display` render-op
    // protocol. Opt-in, never in the prelude.
    (
        "editor/ansi",
        include_str!("../../../../std/editor/ansi.blsp"),
    ),
    // Sets as a library over maps (ADR-062): a set is a map of `element → true`,
    // so membership/elements/size reuse `contains?`/`keys`/`count`; the module
    // adds `set`/`conj`/`disj`/`union`/`intersection`/`difference`/`subset?`.
    // Opt-in, never in the prelude (no `#{…}` literal / distinct type yet).
    ("set", include_str!("../../../../std/set.blsp")),
    // The interactive REPL line editor (ADR-052): `highlight` is the pure lexical
    // syntax-highlighter / bracket-matcher / signature + completion scanners;
    // `lineedit` is the raw-mode, emacs-style editor built on it + the inline
    // `term-*` seam. Both opt-in, never in the prelude; `repl` requires them.
    // `highlight`/`lineedit` stay in CORE: they are reusable UI a shipped app may
    // `require` (the editor's minibuffer reuses `std/lineedit`'s core), not just
    // REPL plumbing — so a lean release keeps them.
    (
        "editor/highlight",
        include_str!("../../../../std/editor/highlight.blsp"),
    ),
    // Generic tree-sitter language services (`fontify` + structural motions) over
    // the `tree-sitter-parse` builtin's positioned CST — the foreign-language
    // analogue of `sexp`+`highlight`. Pure UI a shipped editor `require`s for its
    // ruby/elixir/… modes (ROADMAP §C), so it stays in CORE; opt-in, never prelude.
    (
        "editor/treesit",
        include_str!("../../../../std/editor/treesit.blsp"),
    ),
    // Lexical Markdown highlighter — the `highlight` analogue for `.md` buffers
    // (`markdown-spans` → `[start end face]` spans, ADR-092). Pure UI a shipped app
    // may `require` (myedit's markdown-mode), so it stays in CORE alongside
    // `highlight`/`lineedit`; opt-in, never in the prelude.
    (
        "editor/markdown",
        include_str!("../../../../std/editor/markdown.blsp"),
    ),
    // Lexical `.env` and Dockerfile highlighters, the dotenv/Dockerfile analogues of
    // `markdown` (`env-spans` / `dockerfile-spans` → `[start end face]` spans). Pure
    // UI a shipped app may `require` (myedit's env-/docker-mode); CORE, like markdown.
    (
        "editor/dotenv",
        include_str!("../../../../std/editor/dotenv.blsp"),
    ),
    (
        "editor/dockerfile",
        include_str!("../../../../std/editor/dockerfile.blsp"),
    ),
    (
        "editor/lineedit",
        include_str!("../../../../std/editor/lineedit.blsp"),
    ),
    ("format", include_str!("../../../../std/format.blsp")),
];

/// Dev/tooling modules — baked in only under the `dev-tools` feature (the dev
/// `brood`/`nest` + tests). A `nest release` lean runtime
/// (`--no-default-features`) omits them, so a shipped app carries no test
/// framework, process observer, MCP/doc/hot-reload tooling, or interactive REPL
/// (ADR-038, docs/release.md). `project` stays in CORE — it boots the bundle;
/// `lineedit`/`highlight` stay too (reusable UI, e.g. the editor's minibuffer).
#[cfg(feature = "dev-tools")]
const DEV_MODULES: &[(&str, &str)] = &[
    // The test framework — `deftest`/`describe`/`assert=`/`is`. Never shipped.
    ("test", include_str!("../../../../std/tool/test.blsp")),
    // Doc generation (`nest doc`) — tooling, not runtime.
    ("docs", include_str!("../../../../std/tool/docs.blsp")),
    // Generate editor syntax grammars (VS Code TextMate, Emacs font-lock) from the
    // language's own `(special-forms)` — one source of truth, no drift (ADR-092).
    ("grammar", include_str!("../../../../std/tool/grammar.blsp")),
    // The process viewer / debug tooling (`nest observe`, `(observe)`).
    (
        "observer",
        include_str!("../../../../std/tool/observer.blsp"),
    ),
    // The hot-reload file watcher — a dev-loop convenience.
    ("reload", include_str!("../../../../std/tool/reload.blsp")),
    // The Model Context Protocol tool surface — `(mcp-tools)` returns the
    // catalogue the `nest mcp` dispatcher reads (ADR-036, docs/mcp.md, step 3).
    ("mcp", include_str!("../../../../std/tool/mcp.blsp")),
    // The read-eval-print loop itself, written in Brood (`(require 'repl)`):
    // policy over the `read-line`/`eval-string`/`pr-str` primitives. The Rust
    // binaries (`brood`, `nest repl`) just bootstrap into `(repl-run)`. A shipped
    // app runs its own `:main`, never the REPL.
    ("repl", include_str!("../../../../std/tool/repl.blsp")),
];

/// Empty in a lean (`--no-default-features`) release runtime — the dev modules
/// above are not compiled in at all (their `include_str!` never runs).
#[cfg(not(feature = "dev-tools"))]
const DEV_MODULES: &[(&str, &str)] = &[];

/// Baked-in reference *documents* (markdown), the counterpart to
/// [`EMBEDDED_MODULES`] for non-module text. `(%builtin-doc 'brood-for-claude)`
/// returns the language guide that `nest new` scaffolds into each new project,
/// so a freshly-scaffolded project is self-contained without depending on a
/// Brood install path.
const EMBEDDED_DOCS: &[(&str, &str)] = &[
    (
        "brood-for-claude",
        include_str!("../../../../docs/brood-for-claude.md"),
    ),
    // The Claude Code skill that `nest new` drops into each project's
    // `.claude/skills/`, so an AI assistant editing the project auto-loads the
    // Brood-writing rules. The full reference is `brood-for-claude`; this is the
    // short triggerable checklist (`SKILL.md` frontmatter + the LLM traps).
    // Canonical source lives here in `docs/` (a tracked path); the repo's own
    // `.claude/skills/writing-brood/SKILL.md` is a local symlink to it — `.claude/`
    // is gitignored, and a compile-time `include_str!` must not depend on an
    // untracked path (it would break a fresh clone's build).
    (
        "writing-brood-skill",
        include_str!("../../../../docs/writing-brood-skill.md"),
    ),
];

/// Coerce a (symbol | keyword | string) name argument to its spelling, the shape
/// every embedded-source lookup accepts. `None` for any other value.
pub(super) fn embedded_name(heap: &Heap, v: Value) -> Option<String> {
    match v {
        Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    }
}

/// The lookup body shared by `%builtin-module` and `%builtin-doc`: coerce the
/// (symbol | keyword | string) name argument, find it in `table`, return the
/// baked-in source as a fresh string (or `nil` if absent). `who`/`label` are
/// used only in the type-error message.
pub(super) fn lookup_embedded(
    args: &[Value],
    heap: &mut Heap,
    table: &[(&str, &str)],
    who: &'static str,
    label: &'static str,
) -> LispResult {
    let v = arg(args, 0);
    let name = match embedded_name(heap, v) {
        Some(name) => name,
        None => return Err(LispError::wrong_type(heap, who, label, v)),
    };
    match table.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok(heap.alloc_string(src)),
        None => Ok(Value::nil()),
    }
}

/// `(%builtin-module name)` — the source of a baked-in std module as a string,
/// or nil if there is none. Mechanism only: `require` (Brood) consults this
/// before searching the load-path.
pub(super) fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Core modules first, then dev/tooling modules (absent in a lean release
    // runtime). Both go through `lookup_embedded`, which also validates the arg.
    let found = lookup_embedded(args, heap, CORE_MODULES, "%builtin-module", "module name")?;
    if !matches!(found, Value::Nil) {
        return Ok(found);
    }
    let found = lookup_embedded(args, heap, DEV_MODULES, "%builtin-module", "module name")?;
    if !matches!(found, Value::Nil) {
        return Ok(found);
    }
    // Not a baked-in std module — consult a mounted release bundle (the app's
    // own modules + bundled deps), so `require` resolves them with no change to
    // its load-path logic (ADR-038). The arg type was already validated above.
    let name = match embedded_name(heap, arg(args, 0)) {
        Some(name) => name,
        None => return Ok(Value::nil()),
    };
    match crate::bundle::mounted() {
        Some(b) => match b.module_src(&name) {
            Some(src) => Ok(heap.alloc_string(src)),
            None => Ok(Value::nil()),
        },
        None => Ok(Value::nil()),
    }
}

/// `(%bundled?)` — true when this executable is a release bundle (an app built
/// by `nest release`), false for a plain `brood`/`nest` runtime.
pub(super) fn bundled_p(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(crate::bundle::is_bundled()))
}

/// `(%bundle-manifest)` — the embedded `project.blsp` source of a release
/// bundle, or nil when not bundled.
pub(super) fn bundle_manifest(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::bundle::mounted() {
        Some(b) => Ok(heap.alloc_string(&b.manifest)),
        None => Ok(Value::nil()),
    }
}

/// `(%bundle-module-names)` — the list of module names (filename stems) embedded
/// in a release bundle, or nil when not bundled.
pub(super) fn bundle_module_names(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::bundle::mounted() {
        Some(b) => {
            let items: Vec<Value> = b.module_names().map(|n| heap.alloc_string(n)).collect();
            Ok(heap.list(items))
        }
        None => Ok(Value::nil()),
    }
}

/// `(%builtin-doc name)` — the source of a baked-in reference document as a
/// string, or nil if there is none. Used by `nest new` to scaffold the language
/// guide into each new project.
pub(super) fn builtin_doc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(args, heap, EMBEDDED_DOCS, "%builtin-doc", "doc name")
}

pub(super) fn apply_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity(
            "apply: expected a function and an argument list",
        ));
    }
    // Bind `last` after the guard so the slice indexing below is robust to
    // refactors of the guard: anyone moving / tightening it can't accidentally
    // leave a bare `args[args.len() - 1]` indexing into an empty slice.
    let last = args.len() - 1;
    // The spliced final arg may be a lazy seq-view (`(apply f (map g xs))`) whose
    // realisation re-enters `eval` — a safepoint that can collect and *relocate*
    // LOCAL handles. So the callee `f` and the spliced middle args must be rooted
    // across the realise and re-read after, never trusted as pre-safepoint copies
    // (the re-read discipline ADR-114 requires of any Rust glue holding a LOCAL
    // handle across a GC-capable call; mirrors `prim_eq` / `range_reduce_slow`).
    // Today the only native caller (`%range-reduce` via `apply_value`) never passes
    // a seq-view here, so the realise branch is latent — but the rooting keeps the
    // invariant intact for any future Rust HOF that does.
    heap.root_scope(|heap| {
        let f_r = heap.root(args[0]);
        let mid_roots: Vec<_> = args[1..last].iter().map(|&v| heap.root(v)).collect();
        // `seq_items` can't run a seq-view's transducer, so realise it first.
        let tail = match args[last] {
            sv @ Value::SeqView(_) => realize_seqview(heap, env, sv)?,
            other => other,
        };
        // Re-read across the (possible) collection above before use.
        let f = heap.read_root(f_r);
        let mut argv: Vec<Value> = mid_roots.iter().map(|&r| heap.read_root(r)).collect();
        argv.extend(heap.seq_items(tail)?);
        // Run the target through the active engine (the VM when on), so `apply`-as-a-value
        // — `(map apply …)`, `(reduce apply …)`, apply stored in data — runs its callee
        // compiled, consistent with a direct `(apply f …)` call. This is safe against the
        // `(apply f …)`-driven tail recursion that once forced the tree-walker here
        // (`apply_tail_recursion_does_not_overflow`): a **direct** `apply` call is unfolded
        // by the VM's `dispatch` (it matches the resolved callee, so even `apply` bound to
        // another name unfolds) and TCO'd by the driver, so it never reaches this native;
        // `apply_builtin` is now only hit when a *native* HOF invokes `apply` per element,
        // which loops rather than tail-recurses — one `apply_engine` frame per call, never
        // accumulating. (Deep non-tail recursion in the callee is bounded by the VM's
        // `MAX_BC_FRAMES` guard, not the native stack.)
        apply_engine(heap, f, &argv, env)
    })
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
/// by `(check-project)` in `std/project.blsp` for the `nest test` / `nest run`
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
/// the `(special-forms)` primitive (so `std/highlight.blsp` highlights from this
/// list) and from the LSP (`semantic_tokens` / `completion` import it rather than
/// keeping a copy), so the runtime and the tooling can't drift. Mirrors
/// `brood.el`'s `brood-special-forms` plus the `def`-family heads.
pub const SPECIAL_FORMS: &[&str] = &[
    kw::IF,
    kw::DO,
    kw::DEF,
    kw::FN,
    kw::LAMBDA,
    kw::LET,
    kw::LETREC,
    kw::QUOTE,
    kw::QUASIQUOTE,
    kw::DEFMACRO,
    kw::DEFN,
    kw::DEFDYN,
    kw::DEFMODULE,
    kw::WHEN,
    kw::UNLESS,
    kw::COND,
    kw::AND,
    kw::OR,
    kw::MATCH,
    kw::MATCH_STAR,
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

// ---------- errors / control ----------

/// `(%make-macro f)` — tag the closure `f` as a macro: the expander calls it on
/// the *unexpanded* argument forms and splices the result in place of the call.
/// The `defmacro` macro (std/prelude.blsp) lowers to this, so macro definition is
/// plain Brood over a one-line primitive rather than its own core special form.
pub(super) fn make_macro(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Fn(id) => Ok(Value::macro_(id)),
        other => Err(LispError::type_err(format!(
            "%make-macro: expected a fn, got {}",
            value::tag(other).name()
        ))),
    }
}

pub(super) fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Err(LispError::thrown(arg(args, 0), heap))
}

/// `(%force-panic [msg])` — debug-only. Deliberately panics from a primitive,
/// so tests can exercise the host-side `catch_unwind` boundary (currently the
/// MCP server's `call_tool`). Not a Brood-clean error path — this *is* a Rust
/// `panic!`; if no host catches it, the process dies. There's no Brood
/// reason to call this outside the regression test.
#[cfg(debug_assertions)]
pub(super) fn force_panic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let msg = match args.first() {
        Some(Value::Str(id)) => heap.string(*id).to_string(),
        Some(other) => printer::display(heap, *other),
        None => "%force-panic invoked (no message)".to_string(),
    };
    panic!("{}", msg);
}

/// `(%blob-ptr s)` — debug-only. The raw `SharedBlob` address backing `s`,
/// as an integer (for identity comparison across processes). `nil` for
/// inline (small) strings and PRELUDE/RUNTIME handles.
#[cfg(debug_assertions)]
pub(super) fn blob_ptr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_ptr(id)
            .map(|p| Value::int(p as i64))
            .unwrap_or(Value::nil())),
        other => Err(LispError::type_err(format!(
            "%blob-ptr: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

/// `(%blob-strong-count s)` — debug-only. Current `Arc::strong_count` for
/// the `SharedBlob` backing `s`. `nil` for inline / non-LOCAL strings.
/// Approximate under live concurrent senders/receivers (the count moves);
/// stable when callers are quiescent (what the leak-check test asserts).
#[cfg(debug_assertions)]
pub(super) fn blob_strong_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_strong_count(id)
            .map(|n| Value::int(n as i64))
            .unwrap_or(Value::nil())),
        other => Err(LispError::type_err(format!(
            "%blob-strong-count: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

// ---------- processes ----------

pub(super) fn spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pid = crate::process::spawn(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
}

/// `(%spawn-link thunk)` — atomic `spawn` + `link`: the new child is linked to the
/// caller *before* it runs, so its exit reason is delivered reliably even on an instant
/// exit (no spawn→link `:noproc` race). The `spawn-link` macro wraps an expression.
pub(super) fn spawn_link(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pid = crate::process::spawn_linked(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
}

/// `(%spawn-named name thunk)` — idempotent named spawn. If `name` (a
/// keyword or symbol) is currently registered to a still-alive pid, return
/// that pid and **do not** spawn — `thunk` is never evaluated. Otherwise,
/// drop any stale registration, spawn the thunk as a new green process,
/// register it under `name`, and return the new pid.
///
/// The check-or-spawn step is atomic under `NAMES`'s write lock — two
/// concurrent `(spawn :name …)` calls can't both spawn; the loser sees
/// the winner's pid. The user-facing `(spawn name expr)` macro wraps an
/// expression into a thunk the same way `(spawn expr)` does, so the
/// expression's free locals are captured lexically (ADR-033).
pub(super) fn spawn_named(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) | Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%spawn-named",
                "keyword or symbol",
                v,
            ))
        }
    };
    let thunk = arg(args, 1);
    if !matches!(thunk, Value::Fn(_)) {
        return Err(LispError::wrong_type(
            heap,
            "%spawn-named",
            "function",
            thunk,
        ));
    }
    // `spawn_or_get`'s spawner is fallible — `?` propagates a real
    // `LispError` if `process::spawn` rejects the thunk (defensive: with the
    // `Value::Fn(_)` type-check above, that shouldn't fire today, but a
    // future change to `promote`/`spawn` won't silently panic).
    let pid = crate::dist::spawn_or_get(name, || crate::process::spawn(heap, thunk))?;
    Ok(crate::process::pid_value(pid))
}

pub(super) fn send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send(heap, arg(args, 0), arg(args, 1))?;
    Ok(Value::nil())
}

/// `(exit pid reason)` — send an exit signal to a local green process (Erlang
/// `exit/2`). `reason = :kill` is the untrappable hard kill (dies at its next
/// reduction tick, or now if parked); any other reason is the soft signal (dies at
/// its next `receive`). Returns nil. A no-op for a dead/unknown pid.
pub(super) fn exit_proc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let reason = crate::process::to_message(heap, arg(args, 1))?;
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::exit(id, reason);
            Ok(Value::nil())
        }
        // Cross-node exit (ADR-077): ship a non-link `Frame::Exit` routed to the
        // peer's `scheduler::exit` (kill-style, like the local path).
        Value::Pid { node, id } => {
            crate::dist::exit_remote(node, id, reason);
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err("exit: first argument must be a pid")),
    }
}

/// `(link pid)` — symmetrically link the current process and `pid`, local or
/// remote (ADR-077). A cross-node link ships a `Frame::Link`; either side's death
/// reaches the other, and a net-split fires `:noconnection`. Returns nil.
pub(super) fn link_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::link_self(id);
            Ok(Value::nil())
        }
        Value::Pid { node, id } => {
            crate::dist::link_remote(node, id, crate::process::self_pid());
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err("link: argument must be a pid")),
    }
}

/// `(unlink pid)` — drop the link between the current process and `pid` (local or
/// remote).
pub(super) fn unlink_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::unlink_self(id);
            Ok(Value::nil())
        }
        Value::Pid { node, id } => {
            crate::dist::unlink_remote(node, id, crate::process::self_pid());
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err("unlink: argument must be a pid")),
    }
}

/// `(trap-exit on)` — set the current process's `trap_exit` flag; return the
/// previous value. Only `nil`/`false` are falsy (the language truthiness rule).
pub(super) fn trap_exit_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let on = !matches!(arg(args, 0), Value::Nil | Value::Bool(false));
    let prev = crate::process::set_trap_exit(crate::process::self_pid(), on);
    Ok(Value::boolean(prev))
}

/// `(monitor pid)` — watch `pid`; returns a monitor `ref`. The caller receives
/// `[:down <ref> <pid> <reason>]` when `pid` dies (immediately, reason `:noproc`,
/// if it is already dead).
pub(super) fn monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Local pid: in-process registration, returns a fresh mref.
            Ok(crate::process::monitor(id))
        }
        Value::Pid { node, id } => {
            // Remote pid: same shape — mint a mref, register *here* (so
            // demonitor can find it later, and net-split can fire
            // `:noconnection`), and ship a `Frame::Monitor` to the peer
            // which routes through the same `process::add_monitor` on the
            // far side.
            let mref = crate::process::next_ref();
            let watcher = crate::process::self_pid();
            crate::dist::monitor_remote(node, id, watcher, mref);
            Ok(Value::ref_(mref))
        }
        // `{:name n :node node}` address: resolve to a pid via `whereis` and
        // monitor that pid. Only the local-node case is supported — a remote
        // `{:name :node}` address has no protocol to resolve the name on the
        // far side at monitor time, so we redirect the user to ship the pid
        // directly. Documented in `docs/primitives.md`.
        Value::Map(mid) => {
            let (name, node) = crate::process::read_name_address(heap, mid)?;
            if crate::dist::is_local(node) {
                match crate::dist::whereis(name) {
                    Some(pid) => Ok(crate::process::monitor(pid)),
                    // Unregistered name: behave as if the pid were already
                    // dead — fire :noproc immediately. `process::monitor`
                    // already does this for an unknown local pid, so route
                    // through it with a fresh-but-dead id placeholder.
                    None => Ok(crate::process::monitor(u64::MAX)),
                }
            } else {
                Err(LispError::type_err(
                    "monitor: remote {:name :node} addresses aren't resolvable for monitor — pass the pid",
                ))
            }
        }
        _ => Err(LispError::type_err(
            "monitor: first argument must be a pid or a {:name :node} address",
        )),
    }
}

/// `(demonitor mref)` — drop the monitor created by `(monitor …)`. Tries the
/// local table first; if the mref isn't there it must have been on a remote
/// peer, so a `Frame::Demonitor` is fanned out to every connected peer that
/// holds a pending remote monitor with this watcher + mref.
pub(super) fn demonitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(n) => {
            // Local first (in-process MONITORS table).
            crate::process::demonitor(n);
            // Then ask any peer holding this mref to drop their watcher.
            // We scan PENDING_REMOTE for matching entries and `Demonitor` each
            // unique peer once. The same `process::drop_monitor` predicate the
            // local demonitor used is reused on the far side via the frame
            // handler.
            crate::process::demonitor_remote_fanout(n);
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err(
            "demonitor: argument must be a monitor ref",
        )),
    }
}

/// `(%receive matcher timeout on-timeout)` — the selective-receive primitive the
/// `receive` macro (`std/prelude.blsp`) expands to. See `crate::process::receive_match`.
pub(super) fn receive_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_match(heap, arg(args, 0), arg(args, 1), arg(args, 2))
}

pub(super) fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(crate::process::pid_value(crate::process::self_pid()))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
pub(super) fn make_ref(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::ref_(crate::process::next_ref()))
}

// ----- distributed nodes -----------------------------------------------------

/// Coerce a node/name argument (a keyword or symbol) to its interned `Symbol`.
/// Goes through the same `wrong_type` formatter as the other `expect_*`
/// helpers — pre-fix this one used `type_err` and lost the offending value
/// from the message, the one expect-family inconsistency the review flagged.

pub(super) fn expect_node_name(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "keyword or symbol",
        Value::Keyword(s) => s,
        Value::Sym(s) => s,
    )
}

/// `(node-start name "host:port" cookie)` — name this runtime and listen for peer
/// nodes. Returns the node name.
/// `(%node-listen name addr cookie)` — the listen mechanism behind the prelude's
/// `node-start`. `addr` carries the transport (`"unix:PATH"` / `"tcp:HOST:PORT"`);
/// the path/cookie/transport policy lives in `std/prelude.blsp` (ADR-068).
pub(super) fn node_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "%node-listen", arg(args, 0))?;
    let addr = expect_string(heap, "%node-listen", arg(args, 1))?;
    let cookie = expect_string(heap, "%node-listen", arg(args, 2))?;
    crate::dist::node_listen(name, &addr, cookie).map_err(|e| {
        LispError::runtime(format!("node-start: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::keyword(name))
}

/// `(%node-also-listen addr)` — add another listener to an already-started node
/// (dual-listen, ADR-074). `addr` carries the transport (`"unix:PATH"` /
/// `"tcp:HOST:PORT"`); shares the node's existing identity + cookie.
pub(super) fn node_also_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let addr = expect_string(heap, "%node-also-listen", arg(args, 0))?;
    crate::dist::node_also_listen(&addr).map_err(|e| {
        LispError::runtime(format!("node-also-listen: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::nil())
}

/// `(%node-connect peer addr)` — the dial mechanism behind the prelude's
/// `connect`. `peer` is the expected node name (self-guard + de-dup); `addr`
/// carries the transport. Returns the peer's authoritative node name.
pub(super) fn node_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let peer = expect_node_name(heap, "%node-connect", arg(args, 0))?;
    let addr = expect_string(heap, "%node-connect", arg(args, 1))?;
    let real = crate::dist::node_connect(peer, &addr).map_err(|e| {
        LispError::runtime(format!("connect: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::keyword(real))
}

/// `(random-token n)` — `n` cryptographically-strong random bytes from the OS
/// RNG, hex-encoded into a `2n`-char string. The CSPRNG is mechanism (Rust); the
/// node cookie's generation policy is Brood (`node-cookie`, ADR-068).
pub(super) fn random_token(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "random-token", arg(args, 0))?;
    if !(0..=4096).contains(&n) {
        return Err(LispError::runtime(
            "random-token: byte count must be in 0..=4096",
        ));
    }
    let mut bytes = vec![0u8; n as usize];
    getrandom::fill(&mut bytes)
        .map_err(|e| LispError::runtime(format!("random-token: OS RNG unavailable: {e}")))?;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    Ok(heap.alloc_string(&s))
}

/// `(spit-private path s)` — write `s` to `path` with owner-only (`0600`)
/// permissions, creating the parent directory if needed. The private-by-default
/// write a secret needs (`spit` leaves a world-readable file); the cookie-file
/// policy that uses it is Brood (`node-cookie`, ADR-068).
pub(super) fn spit_private(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = expect_string(heap, "spit-private", arg(args, 0))?;
    let content = expect_string(heap, "spit-private", arg(args, 1))?;
    let err = |e: std::io::Error| {
        LispError::runtime(format!("spit-private: {path}: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(err)?;
    // `.mode` only applies on *create*; enforce 0600 on a pre-existing file too.
    let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    f.write_all(content.as_bytes()).map_err(err)?;
    Ok(Value::nil())
}

/// `(register name pid)` — bind a local name so peers can address this process by
/// `{:name name :node this-node}` before they hold its pid. Returns the pid.
pub(super) fn register_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "register", arg(args, 0))?;
    match arg(args, 1) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::dist::register(name, id);
            Ok(Value::pid(node, id))
        }
        Value::Pid { .. } => Err(LispError::type_err(
            "register: can only register a local pid",
        )),
        _ => Err(LispError::type_err(
            "register: second argument must be a pid",
        )),
    }
}

/// `(node-name)` — this runtime's node name (`:nonode` until `node-start`).
pub(super) fn node_name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::keyword(crate::dist::local_node()))
}

/// `(whereis name)` — the **local** pid registered under `name`, or `nil`.
/// Lets idempotent bootstrap shapes test for "is this server already running
/// here?" before re-`spawn`ing — see `remote-spawn` in `std/prelude.blsp`.
/// A remote-side registration isn't visible here; this is a strictly local
/// lookup over the `NAMES` table.
pub(super) fn whereis_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "whereis", arg(args, 0))?;
    match crate::dist::whereis(name) {
        Some(id) => Ok(Value::pid(crate::dist::local_node(), id)),
        None => Ok(Value::nil()),
    }
}

/// `(monitor-node name)` — the calling process is sent `[:nodedown name]` when a
/// link to `name` goes down (heartbeat timeout or clean close). Returns the name.
pub(super) fn monitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "monitor-node", arg(args, 0))?;
    crate::dist::monitor_node(name, crate::process::self_pid());
    Ok(Value::keyword(name))
}

/// `(demonitor-node name)` — cancel the calling process's node monitor for `name`.
/// A no-op if no monitor is registered. Returns `nil`.
pub(super) fn demonitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "demonitor-node", arg(args, 0))?;
    crate::dist::demonitor_node(name, crate::process::self_pid());
    Ok(Value::nil())
}

/// `(disconnect name)` — drop the link to peer `name` now (Erlang's
/// `disconnect_node`). Returns `true` if a link existed, `false` otherwise.
pub(super) fn disconnect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "disconnect", arg(args, 0))?;
    Ok(Value::boolean(crate::dist::disconnect(name)))
}

/// `(nodes)` — a list of currently connected peer node names.
pub(super) fn nodes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = crate::dist::connected_nodes()
        .into_iter()
        .map(Value::Keyword)
        .collect();
    Ok(heap.list(names))
}

/// `(spawn-count)` — how many green processes have been spawned since the program
/// started. (Green processes are cheap coroutines, not OS threads — step 4b.)
pub(super) fn spawn_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::spawn_count() as i64))
}

/// `(peak-threads)` — high-water mark of processes running *simultaneously*
/// (bounded by the worker-pool size); how much parallelism was actually reached.
pub(super) fn peak_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::peak_threads() as i64))
}

/// `(worker-threads)` — size of the scheduler's worker-thread pool that runs the
/// green processes (≈ `nproc`, or the `-j` setting); 0 until the first spawn.
pub(super) fn worker_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::worker_threads() as i64))
}

/// `(steal-count)` — how many fresh processes the scheduler work-stole across
/// worker threads since program start. A diagnostic of how much the pool had to
/// rebalance; 0 means placement-at-spawn kept it even.
pub(super) fn steal_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::steal_count() as i64))
}

/// `(list-processes)` — every currently-live local pid as a `Pid` value
/// (carrying this runtime's node identity, so the list is `send`-routable as
/// returned). Order is unspecified; sort by `.id` if you need stability.
/// Used by agents / the `nest mcp` `processes` tool to enumerate what's been
/// spawned in the session.
pub(super) fn list_processes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = crate::process::list_local_pids()
        .into_iter()
        .map(crate::process::pid_value)
        .collect();
    Ok(heap.list(items))
}

/// `(%isolate thunk)` — call `thunk` (no args) with a *private copy* of the
/// runtime's global bindings: any `def` it makes is rolled back when it
/// returns, so it cannot affect other code. The test framework wraps each
/// `:isolated` test in this so a test's definitions never leak to another test.
/// Restores the bindings even if the thunk raises (the error then propagates).
///
/// This only isolates *bindings* — the shared code slabs and the symbol interner
/// still grow (memory, not behaviour; there's no GC yet) — and it is sound only
/// with no other process mutating globals concurrently, which the runner ensures
/// by running isolated tests alone.

pub(super) fn isolate(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let saved = heap.snapshot_globals();
    // Pids alive before the run, to tell apart the ones the thunk spawns.
    let before: std::collections::HashSet<u64> =
        crate::process::list_local_pids().into_iter().collect();
    let result = apply_engine(heap, thunk, &[], env);
    // Reap processes the thunk spawned and left running, BEFORE the wholesale
    // global restore below. Otherwise an orphan still running the test's code (a
    // server it spawned but never stopped) looks up a global the test `def`'d,
    // finds it gone after the swap, and dies with a bogus `unbound symbol` (the
    // flaky-suite race). Kill the newcomers, then **yield** until they deregister
    // — `crate::process::yield_now`, NOT `std::thread::sleep`: this runs inside the
    // isolated unit's own green process, so a thread sleep would freeze its worker
    // and starve any orphan pinned to that same worker. Bounded so a wedged orphan
    // can't hang the run.
    let spawned: std::collections::HashSet<u64> = crate::process::list_local_pids()
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    if !spawned.is_empty() {
        let kill = crate::process::Message::Keyword(crate::core::value::intern(
            crate::process::keywords::KILL,
        ));
        for &pid in &spawned {
            // Unlink the child from THIS isolate runner before killing it. A child the
            // thunk `spawn-link`ed is symmetrically linked to us, so a bare
            // `(exit pid :kill)` would propagate `:killed` back through the link and
            // kill the runner itself — even though we're only cleaning up leftovers.
            // Dropping the link first lets the reap take down any straggler (e.g. a
            // server whose async `(stop …)` hasn't finished dying yet) without taking us
            // with it. Best-effort + a no-op for an unlinked child. (Fixes a capture-mode
            // flake where the stop-vs-reap race left a linked server alive at reap; §8.4.)
            crate::process::unlink_self(pid);
            crate::process::exit(pid, kill.clone());
        }
        for _ in 0..10_000 {
            if !crate::process::list_local_pids()
                .into_iter()
                .any(|p| spawned.contains(&p))
            {
                break;
            }
            crate::process::yield_now();
        }
    }
    heap.restore_globals(saved);
    result
}

pub(super) fn try_catch(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let handler = arg(args, 1);
    // The thunk runs through `apply`, which can collect at ANY eval depth
    // (ADR-061). On the error path we still need `handler` and `env` afterwards,
    // so root them on the operand stack across the thunk and re-read the
    // relocated handles. (The thrown value / built error map is fresh after the
    // unwind — no safepoint runs while an `Err` propagates — so it needs no
    // rooting.) This is the `(try (loop) (catch e …))` supervised-server shape.
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_root(handler);
    heap.push_env_root(env);
    let outcome = apply_engine(heap, thunk, &[], env);
    let handler = heap.root_at(vb);
    let env = heap.env_root_at(eb);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    match outcome {
        Ok(value) => Ok(value),
        // A control signal (a `receive` suspend, ADR-100 §7) is **not** an error —
        // re-raise it untouched so it reaches the bytecode driver / scheduler. `%try`
        // must never catch it: it isn't a `throw`/error, and unwinding to the handler
        // here would discard the captured continuation the suspend means to resume.
        Err(e) if e.is_control() => Err(e),
        Err(e) => {
            // The catch sees:
            //   * the user-thrown value verbatim, if there is one (preserves the
            //     "throw shape == catch shape" contract — `(throw 42)` → 42);
            //   * **a structured map** for any built-in error, so Brood code (and
            //     agents via MCP) can `(case (get e :kind) :unbound …)` without
            //     parsing strings (`docs/llm-native.md` §4). Shape on
            //     `LispError::to_value_map`: `{:kind :message [:code] [:file
            //     :line :col] [:hint]}`.
            let caught = match e.payload {
                Some(v) => v,
                None => e.to_value_map(heap),
            };
            apply_engine(heap, handler, &[caught], env)
        }
    }
}

// ----- dynamic variables -----------------------------------------------------
//
// The kernel for `defdyn`/`binding`; the surface macros are in the prelude. A
// dynamic variable's *value* resolves through the per-process binding stack in
// the `Heap` (see `Heap::env_get`), so reads need no primitive here — only the
// declaration, the scoped rebind, and the predicate.

/// `(%declare-dynamic 'name)` — mark a symbol as a dynamic variable, so
/// `binding` will accept it (and `dynamic?` reports it). `defdyn` expands to
/// this plus a plain `def` of the default value.
pub(super) fn declare_dynamic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%declare-dynamic", arg(args, 0))?;
    value::mark_dynamic(sym);
    Ok(Value::symbol(sym))
}

/// `(%in-ns 'foo)` — set the namespace being compiled into (ADR-065). Emitted by
/// the `ns` macro; the resolver pass qualifies subsequent definitions and free
/// references to `foo/…`. Returns the namespace symbol.
pub(super) fn in_ns(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%in-ns", arg(args, 0))?;
    heap.set_compile_ns(Some(sym));
    Ok(Value::symbol(sym))
}

/// `(current-ns)` — the namespace currently being compiled into (a symbol), or
/// `nil` at root. Reflection + a handle for tests (ADR-065).
pub(super) fn current_ns(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.compile_ns().map(Value::Sym).unwrap_or(Value::nil()))
}

/// `(%register-sig 'name 'type)` — record a user-declared `(sig name type)` for the
/// advisory checker. Emitted by the `sig`/`sig!` macros alongside their existing
/// expansion. `name` is qualified to the current namespace *exactly as a `def` head
/// would be* — via [`resolve_reference`](crate::eval::macros::resolve_reference), the
/// same compile-pass entry point `def` uses (own-ns pre-scanned def heads + existing
/// `ns/name` globals qualify; root/prelude names stay bare) — so the key matches the
/// qualified global the call site resolves to. `type` is the raw type-expression form
/// (e.g. `(int -> int)`), stored verbatim on the heap; the checker parses it on read
/// and gives it precedence over inferred/curated sigs. A runtime value-producing call
/// (returns the qualified name), so it composes inside the `sig` macro's expansion.
pub(super) fn register_sig(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%register-sig", arg(args, 0))?;
    let type_value = arg(args, 1);
    // Qualify the name to the current namespace, mirroring how `def` qualifies a
    // definition head — so the store key is the same module-qualified symbol the
    // call site resolves to (intra-module misses the bare file-local ctx; cross-module
    // the sig isn't in the caller's ctx at all).
    let qualified = crate::eval::macros::resolve_reference(heap, name);
    heap.set_declared_sig(qualified, type_value);
    Ok(Value::symbol(qualified))
}

/// `(%refer 'mod subset)` — add `(:use …)` imports to the current file's import
/// table (ADR-065 inc-2). `mod` must already be loaded (the `ns` macro emits a
/// `(require 'mod)` first). `subset` nil → refer every *public* `mod/name` (no
/// `--` private marker, not itself nested); else a seq of bare symbols → refer
/// just those as `mod/name`. Each becomes a bare → qualified entry the resolver
/// consults after the current namespace and before root.
pub(super) fn refer(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mod_sym = expect_symbol(heap, "%refer", arg(args, 0))?;
    let mod_name = value::symbol_name(mod_sym);
    let prefix = format!("{}/", mod_name);
    match arg(args, 1) {
        Value::Nil => {
            // Refer all public names: enumerate the live globals under `mod/`.
            for g in heap.global_symbols() {
                let name = value::symbol_name(g);
                if let Some(bare) = name.strip_prefix(&prefix) {
                    if !bare.is_empty() && !bare.contains('/') && !bare.contains("--") {
                        let bare_sym = value::intern(bare);
                        heap.add_import(bare_sym, g);
                    }
                }
            }
        }
        subset => {
            // Refer just the named symbols as `mod/name` (existence not required —
            // an unbound `mod/name` surfaces as a normal unbound-reference error).
            for item in heap.seq_items(subset)? {
                let bare = expect_symbol(heap, "%refer", item)?;
                let qualified =
                    value::intern(&format!("{}/{}", mod_name, value::symbol_name(bare)));
                heap.add_import(bare, qualified);
            }
        }
    }
    Ok(Value::nil())
}

/// `(%alias module short)` — register a module alias (Elixir-style): a later
/// qualified reference `short/name` resolves to `module/name`. Stored in the import
/// table under the slash-suffixed key `short/`, so it rides the same per-file
/// lifecycle as `%refer`. The `(:alias …)` header emits it. A second `short` for a
/// different module is a loud error (the ambiguous-last-segment case — disambiguate
/// with an explicit `:as`).
pub(super) fn alias(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let module = expect_symbol(heap, "%alias", arg(args, 0))?;
    let short = expect_symbol(heap, "%alias", arg(args, 1))?;
    let key = value::intern(&format!("{}/", value::symbol_name(short)));
    if let Some(prev) = heap.import_of(key) {
        if prev != module {
            return Err(LispError::runtime(format!(
                "alias `{}` is already bound to `{}` — can't also alias `{}`; give one an explicit `:as` name",
                value::symbol_name(short),
                value::symbol_name(prev),
                value::symbol_name(module),
            )));
        }
    }
    heap.add_import(key, module);
    Ok(Value::nil())
}

/// `(dynamic? x)` — true when `x` is a symbol declared dynamic with `defdyn`.
/// A non-symbol is simply not dynamic (no error), so it composes in predicates.
pub(super) fn dynamic_p(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(
        matches!(arg(args, 0), Value::Sym(s) if value::is_dynamic(s)),
    ))
}

/// `(%binding syms vals thunk)` — run `thunk` (no args) with each dynamic var in
/// `syms` bound to the matching value in `vals` for the dynamic extent of the
/// call, restoring the previous bindings on return *or* error. `syms` (a quoted
/// list) and `vals` (a vector) are equal-length sequences built by the `binding`
/// macro — both emitted as unshadowable literals, so a local rebinding of `list`
/// can't break the form. Every name must be declared dynamic (else it's almost
/// certainly a typo — a plain global won't track the rebind). The bindings live
/// in this process's heap, so they don't reach a `spawn`ed child.
pub(super) fn binding(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let syms = heap.seq_items(arg(args, 0))?;
    let vals = heap.seq_items(arg(args, 1))?;
    let thunk = arg(args, 2);
    // Validate every name up front, before pushing anything — so a bad `binding`
    // leaves the dynamic stack untouched rather than half-pushed.
    let mut names = Vec::with_capacity(syms.len());
    for s in &syms {
        let sym = expect_symbol(heap, "binding", *s)?;
        if !value::is_dynamic(sym) {
            return Err(LispError::runtime(format!(
                "binding: {} is not a dynamic variable (declare it with defdyn)",
                value::symbol_name(sym)
            )));
        }
        names.push(sym);
    }
    for (i, &sym) in names.iter().enumerate() {
        heap.push_dynamic(sym, arg(&vals, i));
    }
    let result = apply_engine(heap, thunk, &[], env);
    for _ in 0..names.len() {
        heap.pop_dynamic();
    }
    result
}
