use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::{cst, reader};

use super::numeric::{arg, expect_int, expect_string, expect_symbol};
use super::realize_seqview;
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
    // Route a runtime-evaluated form through the compiling VM when it's enabled, so a form
    // handed to `eval` isn't stuck on the ~10-14× tree-walker (deferred.md #9): `compile::run`
    // compiles what it can and falls back to the tree-walker per-form for anything outside
    // the VM's vocabulary, so semantics are unchanged; only the top-level call (which
    // dispatches into the VM, where a callee's arm compiles and tail-recurses in O(1) stack)
    // stops being interpreted.
    //
    // The full `compile` pass, matching `eval-string` and the file loader — so an eval'd
    // form gets namespace resolution, `(:use …)` imports, `(:alias …)`, privacy enforcement
    // and static-quasiquote lowering, and an eval'd `defn` inside a module defines
    // `mod/name` rather than leaking a bare ROOT global.
    //
    // `compile`'s resolve step qualifies a bare reference only on positive evidence, which
    // a file loader supplies by pre-scanning its def heads (`scan_def_names`) — lookahead a
    // one-form-at-a-time `eval` does not have, so a forward reference to a name a LATER
    // `eval` defines was left bare and missed the qualified global (KI-24). `ns_assume_own`
    // supplies the missing conclusion instead of dropping the pass: a bare name bound
    // nowhere is taken to be this namespace's. A no-op at root, where resolve already is.
    let prev_assume = heap.set_ns_assume_own(true);
    let compiled = crate::eval::macros::compile(heap, arg(args, 0), root);
    heap.set_ns_assume_own(prev_assume);
    let form = compiled?;
    if crate::eval::compile::vm_enabled() {
        crate::eval::compile::run(heap, form, root)
    } else {
        crate::eval::eval(heap, form, root)
    }
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
// ── native per-file scan extract (ADR-119 whole-project check) ───────────────
// `scan-source-extract` replaces the interpreted-Brood CST walk that was the
// dominant cost of a cold whole-project check (~120ms on a 1000-line file vs
// ~2.6ms to parse it natively). Same three outputs as the old
// `project--scan-file-entry`, computed in one Rust pass over the reader's forms.

const SCAN_DEF_HEADS: &[&str] = &[
    "def", "def-", "defn", "defn-", "defmacro", "defdyn", "defonce",
];

/// An **ambient** name — root regardless of the enclosing namespace. Ambient status
/// is a *declaration*, not a spelling: the `defdyn` head declares it (the earmuff
/// convention no longer grants it — see `eval::macros::is_ambient`). This scan runs
/// over unevaluated source, so the head is the evidence; a name declared `defdyn`
/// elsewhere is caught by `is_dynamic`.
fn scan_is_ambient(head: &str, name: &str) -> bool {
    head == crate::core::keywords::DEFDYN
        || crate::core::value::is_dynamic(crate::core::value::intern(name))
}

/// Mirror `project--qualify`: `ns/name`, unless the name is ambient, already
/// qualified, or there's no module namespace.
fn scan_qualify(ns: Option<&str>, head: &str, name: &str) -> String {
    match ns {
        Some(n) if !scan_is_ambient(head, name) && !name.contains('/') => format!("{n}/{name}"),
        _ => name.to_string(),
    }
}

fn scan_sym_name(v: Value) -> Option<&'static str> {
    match v {
        Value::Sym(s) => crate::core::value::symbol_name_opt(s),
        _ => None,
    }
}

/// The head and second element of a list form (nil if it isn't a ≥2-element list).
fn scan_head2(heap: &Heap, f: Value) -> Option<(Value, Value)> {
    if let Value::Pair(id) = f {
        let (car, cdr) = heap.pair(id);
        if let Value::Pair(id2) = cdr {
            return Some((car, heap.pair(id2).0));
        }
    }
    None
}

/// Count every symbol occurrence anywhere in `v` (recursively). Privacy is now a
/// def-site fact with a CLEAN name (ADR-146 step 2), so the unused-private verdict
/// can no longer restrict counting to `--` names — it looks up each private's bare
/// and qualified name, both ordinary symbols. The bare count is project-global, so a
/// name shared across modules reads as "used" (a false negative, never a false
/// positive — safe for the zero-false-positive advisory contract).
fn scan_count_syms(heap: &Heap, v: Value, counts: &mut std::collections::HashMap<String, i64>) {
    match v {
        Value::Sym(s) => {
            if let Some(n) = crate::core::value::symbol_name_opt(s) {
                *counts.entry(n.to_string()).or_insert(0) += 1;
            }
        }
        Value::Pair(id) => {
            let (car, cdr) = heap.pair(id);
            scan_count_syms(heap, car, counts);
            scan_count_syms(heap, cdr, counts);
        }
        Value::Vector(vid) => {
            for it in heap.vector(vid).to_vec() {
                scan_count_syms(heap, it, counts);
            }
        }
        Value::Map(mid) => {
            for (k, val) in heap.map_entries(mid) {
                scan_count_syms(heap, k, counts);
                scan_count_syms(heap, val, counts);
            }
        }
        _ => {}
    }
}

/// `(scan-source-extract src)` → `[counts privs def-names]` for the whole-project
/// check's per-file scan (ADR-119): `counts` a map of each symbol name → occurrence
/// count, `privs` this file's `defn-`/`def-` private top-level defs as `[bare qual]`,
/// `def-names` every top-level def's qualified global key. Malformed input yields an
/// empty extract (parse-tolerant — advisory).
pub(super) fn scan_source_extract(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "scan-source-extract", arg(args, 0))?;
    let forms = reader::read_all(heap, &src).unwrap_or_default();
    // First `(defmodule NAME …)`'s NAME is the file's namespace.
    let ns: Option<String> = forms.iter().find_map(|&f| {
        let (h, n) = scan_head2(heap, f)?;
        (scan_sym_name(h)? == "defmodule").then(|| scan_sym_name(n).map(str::to_string))?
    });
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut def_names: Vec<Value> = Vec::new();
    let mut privs: Vec<Value> = Vec::new();
    for &f in &forms {
        scan_count_syms(heap, f, &mut counts);
        if let Some((h, n)) = scan_head2(heap, f) {
            if let (Some(head), Some(name)) = (scan_sym_name(h), scan_sym_name(n)) {
                if SCAN_DEF_HEADS.contains(&head) {
                    let qual = scan_qualify(ns.as_deref(), head, name);
                    let qv = heap.alloc_string(&qual);
                    def_names.push(qv);
                    // A `defn-`/`def-` head marks a module-private (ADR-146); the name
                    // itself is clean, so privacy is read from the def form, not the name.
                    if head == "defn-" || head == "def-" {
                        let bv = heap.alloc_string(name);
                        privs.push(heap.alloc_vector(vec![bv, qv]));
                    }
                }
            }
        }
    }
    let count_pairs: Vec<(Value, Value)> = counts
        .iter()
        .map(|(k, &c)| (heap.alloc_string(k), Value::int(c)))
        .collect();
    let counts_v = heap.map_from_pairs(count_pairs);
    let privs_v = heap.list(privs);
    let defn_v = heap.list(def_names);
    Ok(heap.alloc_vector(vec![counts_v, privs_v, defn_v]))
}

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
        Symbol | Keyword | Int | Float | Decimal | Ratio | Str | Bool | Nil | Whitespace
        | Comment | Error => {
            let k = match node.kind {
                Symbol => "symbol",
                Keyword => "keyword",
                Int => "int",
                Float => "float",
                Decimal => "decimal",
                Ratio => "ratio",
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
        Quote | Quasi | Unquote | Splice | Pin => {
            let k = match node.kind {
                Quote => "quote",
                Quasi => "quasi",
                Unquote => "unquote",
                Splice => "splice",
                Pin => "pin",
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
        Root | List | Vector | Map | Set => {
            let k = match node.kind {
                Root => "root",
                List => "list",
                Vector => "vector",
                Map => "map",
                Set => "set",
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
        Ratio => "ratio",
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
        Pin => "pin",
        Root => "root",
        List => "list",
        Vector => "vector",
        Map => "map",
        Set => "set",
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
        Symbol | Keyword | Int | Float | Decimal | Ratio | Str | Bool | Nil | Whitespace
        | Comment | Error => {
            let text = heap.alloc_string(node.text(src));
            pairs.push((kw("text"), text));
        }
        // Containers + wrappers carry their (position-annotated) children — trivia
        // included, exactly as `parse-source`, so callers filter what they want.
        Quote | Quasi | Unquote | Splice | Pin | Root | List | Vector | Map | Set => {
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
/// file watcher (`std/tool/reload.blsp`): on the **second** and subsequent visits to
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
    // Namespace bracketing + forward-ref pre-scan, like `load` (ADR-065): a reloaded
    // namespaced file re-establishes its own namespace (its `(defmodule …)` form is
    // re-evaluated below) so its re-saved defs are qualified correctly. `NsLoadScope`
    // restores the caller's ns-state on every exit path (incl. panic).
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let mut scope = crate::eval::macros::NsLoadScope::enter(heap, &form_vals);
    let heap = scope.heap();
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
            .and_then(|f| {
                // Also record the *expanded* form's def sites, so a macro-defined
                // global whose raw head isn't a def*` (a `defrecord` constructor /
                // accessor, a `defability` op) gets a site too — matching the
                // file-runner (`lib.rs`). Without this, cross-file goto-definition
                // on those names finds nothing under `(load …)` / project modules.
                heap.note_definition(f, pos);
                crate::eval::eval(heap, f, root)
            })
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    result.map(|_| Value::Nil)
    // `scope` drops here → the caller's ns-state is restored (also on panic).
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
    // A loaded file starts at the ROOT namespace; its own `(defmodule …)` sets the
    // namespace for the rest of the file (ADR-065). `NsLoadScope` resets compile-ns +
    // imports + this file's forward-ref pre-scan + assume-own, and restores the
    // caller's ns-state on EVERY exit path (normal return, `?`, or a panic unwinding
    // through the load) — so ns state never leaks out of a file and the four ns fields
    // stay in sync. It owns the heap for the load; reach it via `scope.heap()`.
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let mut scope = crate::eval::macros::NsLoadScope::enter(heap, &form_vals);
    let heap = scope.heap();

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
            .and_then(|f| {
                // Also record the *expanded* form's def sites, so a macro-defined
                // global whose raw head isn't a def*` (a `defrecord` constructor /
                // accessor, a `defability` op) gets a site too — matching the
                // file-runner (`lib.rs`). Without this, cross-file goto-definition
                // on those names finds nothing under `(load …)` / project modules.
                heap.note_definition(f, pos);
                crate::eval::eval(heap, f, root)
            })
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    result
    // `scope` drops here → the caller's compile-ns / known-names / imports / assume-own
    // are restored (also on a panic unwinding through the load).
}

/// `(%run-program-file "path")` — run a program **file** as its own green process
/// (ADR-135) and block until it finishes, returning nil (or raising if a top-level form
/// did). Unlike `load` — which tree-walks the file's forms inline, so a top-level
/// `receive` blocks the caller's thread — this drives the file as a real process on a
/// worker in capture mode: a top-level driver talking to a spawned worker uses the
/// userspace direct-handoff path, and top-level `receive`s park-and-capture. It shares
/// this runtime's globals/`*load-path*`, so a preceding `project-setup` (which `def`s the
/// path) is visible to the file's `(require …)`. `nest run FILE` routes here so a run
/// script gets the same fast path as `brood FILE`.
pub(super) fn run_program_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%run-program-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("%run-program-file: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let exit = crate::process::spawn_root_program(heap, &src, Some(path.clone()))
        .map_err(|e| e.or_file(path.clone()))?;
    match exit.wait() {
        Ok(()) => Ok(Value::nil()),
        // The program raised. The error is already file/pos-tagged by the program
        // driver, so render the full report (caret, hint, call trace) and exit 1
        // exactly as `brood FILE` does (run_files) rather than returning an error the
        // caller would re-decorate with a meaningless position inside the generated run
        // script. Restore the terminal first (a TUI that threw before its `term-leave`
        // would otherwise wedge the shell); `process::exit` skips Drop, so do it
        // explicitly — the same no-op-unless-raw call the CLI makes.
        Err(e) => {
            crate::builtins::restore_terminal_on_exit();
            crate::cli_support::report_error(&e);
            std::process::exit(1);
        }
    }
}

/// `(eval-string "src")` — read and evaluate every form in a string against the
/// global environment. Inherits the current namespace (ADR-065): the REPL evaluates
/// each entry through here, so a `(ns foo)` typed at the REPL sticks to later
/// entries. To load a *module* source at the root namespace, use `%load-string`.
pub(super) fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "eval-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, false)
}

/// `(%load-string "src")` / `(%load-string "src" "name")` — the string analogue of
/// `load`: read+eval every form, but bracket the current namespace (reset to root,
/// restore the caller's after), so an embedded module's own `(ns …)` governs it and ns
/// state doesn't leak to the caller. Used by `require-one` for baked-in std modules
/// (ADR-065).
///
/// The optional `name` is what the forms are attributed to for the duration. A baked-in
/// module has no path on disk, and without a name its forms inherit whatever file
/// happened to be loading when the `require` ran — so `std/log`'s lines were reported as
/// the requiring file's, which line coverage caught by crediting a 21-line `main.blsp`
/// with std's line 175. The attribution feeds `CompiledArm::src_file`, hence `:trace`
/// frames too. `require--force` passes the module's real repo-relative path, from
/// `%builtin-module-file` — see [`EmbeddedModule`], which keeps that path in step with
/// the `include_str!` it was baked in from.
pub(super) fn load_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-string", arg(args, 0))?;
    let name = match arg(args, 1) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%load-string", v)?),
    };
    let previous_file = name.map(|n| heap.set_current_file(Some(n)));
    let result = eval_string_inner(heap, env, &src, true);
    if let Some(previous) = previous_file {
        heap.set_current_file(previous);
    }
    result
}

/// `(%load-module-source src file)` — load an **embedded std module**'s source with
/// the reserved-name exemption held (ADR-166).
///
/// Identical to `%load-string` except that the module's own `def`s are permitted to
/// (re)bind reserved names *and* become reserved themselves. Two reasons it has to be
/// a primitive rather than a flag `require` sets and clears in Brood: the exemption
/// must be released even when the load **throws** (a leaked one would silently
/// un-reserve the language for the rest of the process's life), and it must not be
/// reachable as an on/off pair that user code could straddle. `require` uses it for
/// baked-in source only — a project file loaded off `*load-path*` goes through the
/// ordinary `load`, so a package's names are never reserved.
pub(super) fn load_module_source(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-module-source", arg(args, 0))?;
    let name = match arg(args, 1) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%load-module-source", v)?),
    };
    let previous_file = name.map(|n| heap.set_current_file(Some(n)));
    heap.enter_module_load();
    let result = eval_string_inner(heap, env, &src, true);
    heap.leave_module_load();
    if let Some(previous) = previous_file {
        heap.set_current_file(previous);
    }
    result
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
    // No pre-scan on the inheriting path, so a reference to a name a LATER call will
    // define has no evidence to qualify against and would be left bare, missing the
    // module-qualified global (KI-24) — in the REPL that is just typing `(defmodule m)`
    // and then two mutually recursive `defn`s. Tell the resolver to fall back to the
    // current namespace for a name bound nowhere else; a no-op at root. The module-load
    // path sets it *off*: it has the real pre-scan, and a nested load must not inherit
    // an outer `eval`'s assumption.
    let prev_assume = heap.set_ns_assume_own(!reset_ns);
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds.
    let base = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let mut result: LispResult = Ok(Value::nil());
    for i in 0..forms.len() {
        let form = heap.root_at(base + i);
        // Same as `eval_builtin`/the file loader: compile then run on the VM when enabled
        // (deferred.md #9), tree-walker under `BROOD_VM=0`. `compile::run` falls back to the
        // tree-walker per-form, so a form outside the VM's vocabulary still evaluates.
        match crate::eval::macros::compile(heap, form, root).and_then(|f| {
            if crate::eval::compile::vm_enabled() {
                crate::eval::compile::run(heap, f, root)
            } else {
                crate::eval::eval(heap, f, root)
            }
        }) {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(base);
    heap.set_ns_assume_own(prev_assume);
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

/// `(%locals)` / `(%scope)` — the CALLER's in-scope local bindings as a `{:name → value}`
/// map (innermost binding of each name wins; globals excluded). This is the tree-walker
/// fallback: under the VM both spellings are a compiler intrinsic that reads the
/// lexical-scope table directly (see `compile_scope_map`), which is why a compiled arm's
/// call never reaches here. Keyed by the name as a **keyword** to match the intrinsic and
/// the debugger's explicit `:vals`, so a named value overrides a captured local on `merge`
/// and `%eval-in` (which binds keyword- and symbol-keyed entries alike) resolves it.
/// `dev-tools` only (its sole consumer is the `debug` DEV_MODULE).
#[cfg(feature = "dev-tools")]
pub(super) fn locals(_: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let mut seen: Vec<value::Symbol> = Vec::new();
    let mut pairs: Vec<(Value, Value)> = Vec::new();
    let mut cur = Some(env);
    while let Some(e) = cur {
        if e == EnvId::GLOBAL {
            break;
        }
        // Collect this frame's bindings (innermost binding last → scan reversed) into an
        // owned Vec, releasing the borrow before touching `heap` again.
        let (parent, vars) = heap.env_frame_ref(e);
        let frame: Vec<(value::Symbol, Value)> = vars.iter().rev().copied().collect();
        cur = parent;
        for (s, v) in frame {
            if !seen.contains(&s) {
                seen.push(s);
                pairs.push((Value::keyword(s), v));
            }
        }
    }
    Ok(heap.map_from_pairs(pairs))
}

/// `(%eval-in "src" locals-map)` — read + evaluate `src`'s forms in a fresh environment
/// holding `locals-map`'s `{name → value}` bindings over the globals; returns the last
/// result. Lets the debugger evaluate an expression in a paused worker's captured scope,
/// so a breakpoint's locals resolve. GC-safe: a fresh frame per form (used by exactly one
/// `run`, so it can't go stale across a collection); held forms + local values are rooted
/// across the eval. `dev-tools` only.
#[cfg(feature = "dev-tools")]
pub(super) fn eval_in(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%eval-in", arg(args, 0))?;
    let entries: Vec<(value::Symbol, Value)> = match arg(args, 1) {
        Value::Map(mid) => heap
            .map_entries(mid)
            .into_iter()
            .filter_map(|(k, v)| match k {
                Value::Sym(s) | Value::Keyword(s) => Some((s, v)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let forms = reader::read_all(heap, &src)?;
    let fbase = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let lbase = heap.roots_len();
    for &(_, v) in &entries {
        heap.push_root(v);
    }
    let mut result: LispResult = Ok(Value::nil());
    for i in 0..forms.len() {
        let form = heap.root_at(fbase + i);
        let frame = heap.new_env(Some(EnvId::GLOBAL));
        for (j, &(s, _)) in entries.iter().enumerate() {
            let v = heap.root_at(lbase + j);
            heap.env_define(frame, s, v);
        }
        match crate::eval::compile::run(heap, form, frame) {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(fbase);
    result
}

/// One baked-in std module: the `require` key, the source, and the **repo-relative
/// path the source came from**.
///
/// The path exists so a baked module's forms can be attributed to the file they were
/// actually written in. Without it they inherited whatever file happened to be loading
/// when the `require` ran (`%load-string` set none), so a 21-line `src/main.blsp` was
/// credited with `std/log`'s line 175 — in line coverage, and in `:trace` frames, which
/// take their file from the same `CompiledArm::src_file`.
///
/// [`embedded_module!`] derives `path` from the same literal as the `include_str!`, so
/// the two cannot drift: change the file a module loads from and its recorded path
/// follows.
pub(super) struct EmbeddedModule {
    pub key: &'static str,
    pub source: &'static str,
    pub path: &'static str,
}

/// `embedded_module!("log", "std/log.blsp")` — one [`EmbeddedModule`], with the source
/// baked in from `path` and `path` kept as the recorded origin.
macro_rules! embedded_module {
    ($key:expr, $path:literal) => {
        EmbeddedModule {
            key: $key,
            source: include_str!(concat!("../../../../", $path)),
            path: $path,
        }
    };
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
const CORE_MODULES: &[EmbeddedModule] = &[
    // Output ports: the redirectable sink behind print/println — a port is a 1-arg
    // string sink, with `process-port`/`fn-port` + `with-out`/`with-err`. Pairs
    // with the prelude's `*out*`/`*err*` dynamic vars. Opt-in, no dependencies.
    embedded_module!("io", "std/io.blsp"),
    // Fuzzy (subsequence) string matching + ranking: `fuzzy-match` / `fuzzy-filter`,
    // the matcher completion UIs ride on. Pure Brood, no dependencies. Opt-in.
    embedded_module!("fuzzy", "std/fuzzy.blsp"),
    // Plain-text utilities (pure string->string): `fill` greedy word-wraps to a column
    // width — the engine behind an editor's fill-paragraph / M-q, and reusable for
    // wrapping help text or terminal output. No dependencies. Opt-in.
    embedded_module!("text", "std/text.blsp"),
    embedded_module!("project", "std/tool/project.blsp"),
    embedded_module!("coverage", "std/tool/coverage.blsp"),
    embedded_module!("complete", "std/tool/complete.blsp"),
    // `nest new` scaffolding (templates + new-project), split out of `project` so
    // the analysis half stays lean. `(:use project)` for *config-git-init*. Opt-in.
    embedded_module!("scaffold", "std/tool/scaffold.blsp"),
    // The package manager (ADR-037): resolves the manifest's :dependencies into a
    // lock file + load-path entries. Required lazily by `project-setup` only when a
    // project actually declares deps. Opt-in, never in the prelude.
    embedded_module!("package", "std/tool/package.blsp"),
    // TCP sockets (ADR-062): active-socket helpers + a spawn-per-connection
    // server over the non-blocking tcp-* primitives. Opt-in, never in the prelude.
    embedded_module!("net/tcp", "std/net/tcp.blsp"),
    // The file & filesystem library: whole-file/line I/O, directory walking, path
    // helpers — Brood over the fs primitives. Opt-in, never in the prelude.
    embedded_module!("file", "std/file.blsp"),
    // A minimal HTTP/1.0 server (ADR-062) over the tcp + file libraries — request
    // parsing, response rendering, a router, static files. Opt-in.
    embedded_module!("net/http", "std/net/http.blsp"),
    // JSON ↔ Brood data, written entirely in Brood (a recursive-descent parser +
    // encoder over the string primitives; the reader's `\u{}` escape is the
    // codepoint→char mechanism). Opt-in, never in the prelude.
    embedded_module!("json", "std/json.blsp"),
    // WASM component interop (ADR-071/145): load sandboxed native components,
    // call exports (marshalled by WIT types), `use-native` binding. Policy over
    // the `%wasm-*` primitives (feature `wasm`; without it the primitives are
    // unbound and requiring this module errors clearly). Opt-in.
    embedded_module!("wasm", "std/wasm.blsp"),
    // Teach-the-error + intent→idiom lookup (LLM-native errors): explain-error
    // (a stable E-code → summary/causes/fix/example) and find-pattern (an
    // intent → the idiomatic Brood pattern). Curated Brood data; backs the
    // `nest mcp` tools of the same names. Opt-in.
    embedded_module!("explain", "std/tool/explain.blsp"),
    // Supervised node auto-reconnect (dist self-healing): `watch` keeps a peer
    // link alive with exponential-backoff `(connect …)` retries; subscribers get
    // [:nodeup]/[:nodedown]. Pure Brood over connect/monitor-node/nodes. Opt-in.
    embedded_module!("net/reconnect", "std/net/reconnect.blsp"),
    // Server-Sent Events (text/event-stream): a client reader process that streams
    // events to a subscriber's mailbox (pairs with ui's `with-events`) + server-side
    // framing. Pure frame parsing + a thin IO loop over tcp; reuses http's URL/header
    // helpers. Opt-in.
    embedded_module!("net/sse", "std/net/sse.blsp"),
    // The process framework, bundled in the default install (ADR-085 amended —
    // batteries-included, not externalized). `proc/gen` is the gen_server-style
    // server loop (`defprocess` / `spawn-server` / `!` / `gen-call` / `stop`); the
    // core `log` module is a `proc/gen` process. `proc/supervisor` is OTP-style
    // supervision — independent of `proc/gen`, both over the same kernel primitives.
    embedded_module!("proc/gen", "std/proc/gen.blsp"),
    embedded_module!("proc/supervisor", "std/proc/supervisor.blsp"),
    // Process-backed state cell: start/get/update/get-and-update/cast/stop.
    // A thin Brood layer over spawn/send/receive for the common "stateful process" case.
    embedded_module!("proc/agent", "std/proc/agent.blsp"),
    // Order a flat process-info snapshot as a parent→child forest (depth-tagged, DFS
    // by id). A pure, dependency-free transform — CORE, not dev-tools: it's shared by
    // the dev observer's tree sort *and* a shipped app's process list (bedit's
    // *Process List*), so a `nest release` binary needs it baked in.
    embedded_module!("proctree", "std/tool/proctree.blsp"),
    // Run a thunk off the current process with an optional timeout + cancel
    // (ADR-006): `task` (async, tagged-reply handle), `cancel-task`, and the
    // synchronous `await`. Pure Brood over spawn / receive / exit — the generic
    // version of the editor's hand-rolled async-eval watchdog. Opt-in.
    embedded_module!("task", "std/task.blsp"),
    // An async, safe logger (ADR-006): a `proc/gen` process holding a list of
    // backends, each an `io` port + a min level + a formatter. Log calls are casts
    // (fire-and-forget = async); the one process serialises writes (no interleaving)
    // and isolates a backend crash. Opt-in, never in the prelude.
    embedded_module!("log", "std/log.blsp"),
    // Erlang :telemetry-style instrumentation (ADR-106). Handlers run in a dedicated
    // LISTENER process (emit is a fire-and-forget send), so a buggy handler can never
    // crash/hang the emitting process — only the listener, which a throwing handler
    // doesn't even do (caught + detached). The handler table is a `def`-rebound global
    // that survives a listener restart (ADR-013). `span` brackets a body with
    // :start/:stop/:exception events; `forward` runs handler work in your own process.
    // Opt-in, never in the prelude.
    embedded_module!("telemetry", "std/telemetry.blsp"),
    // Date and time utilities (UTC): epoch↔datetime conversion, ISO 8601
    // format/parse, arithmetic, calendar predicates. Pure Brood over `now`.
    embedded_module!("datetime", "std/datetime.blsp"),
    // Hex and Base64 encoding/decoding. Pure Brood over `char->int` /
    // `string->utf8-bytes` / `utf8-bytes->string`. Opt-in, never in the prelude.
    embedded_module!("encoding", "std/encoding.blsp"),
    // Descriptive statistics over numeric sequences: mean, median, stddev,
    // variance, percentile, mode, frequencies. Pure Brood over sort/fold/sqrt.
    embedded_module!("stats", "std/stats.blsp"),
    // Pull-stream protocol + combinators over green processes. Sources: list,
    // fn-generator, range, TCP socket. Transformers: map/filter/take/drop/
    // take-while/chunk/concat/lines. Terminals: fold/to-list/to-vector/
    // for-each/pipe/to-socket. Foundation for the HTTP streaming layer.
    embedded_module!("stream", "std/stream.blsp"),
    // URL encoding/decoding and parsing: percent-encode/decode, query-string
    // encode/decode, parse-url, build-url. Pure Brood over string primitives.
    embedded_module!("url", "std/url.blsp"),
    // CSV parsing and emitting: csv-parse, csv-parse-maps, csv-emit,
    // csv-emit-maps. Handles quoted fields, escaped quotes, \r\n endings.
    embedded_module!("csv", "std/csv.blsp"),
    // RFC 4122 version-4 UUID generation via the OS CSPRNG (random-token).
    // uuid-v4, uuid-nil, uuid?.
    embedded_module!("uuid", "std/uuid.blsp"),
    // {{var}} string templating: render a template string against a data map.
    // render, render-all.
    embedded_module!("template", "std/template.blsp"),
    // The documentation-site renderer: a pure `model -> HTML string` for `nest docs`
    // and hive's per-package doc builds (both feed it the same doc-model shape). CORE,
    // not DEV, because a shipped app (hive) requires it at runtime to render docs.
    embedded_module!("docsite", "std/docsite.blsp"),
    // The function catalogue: bare builtin/prelude name -> functional category, plus the
    // category order/titles. CORE so both `nest docs --all` and a shipped app (hive's
    // /reference) present the categorised language reference from one source.
    embedded_module!("doc-catalog", "std/doc-catalog.blsp"),
    // Purely functional FIFO queue (two-list, amortised O(1)) and min-priority
    // queue (sorted-list, O(n) insert / O(1) pop).
    embedded_module!("queue", "std/queue.blsp"),
    // Multi-valued map: one key may hold multiple values (a map of lists).
    // multimap-assoc, multimap-get, multimap-get-all, multimap-dissoc, …
    embedded_module!("multimap", "std/multimap.blsp"),
    // MD5/SHA-1/SHA-256/SHA-384/SHA-512 + HMAC, all Brood over the two `%digest`
    // / `%hmac` prims (raw bytes); hex/string shaping via bytes->hex; hash-string is djb2.
    embedded_module!("hash", "std/hash.blsp"),
    // gzip/zlib/raw-deflate compression
    // (gzip/gunzip, compress/uncompress, zip/unzip) over the six %gzip/%deflate prims.
    embedded_module!("zlib", "std/zlib.blsp"),
    // LCS-based sequence diff: diff-seq, diff-lines, diff-summary, diff-patch,
    // diff-unified. O(m*n) time/space; suitable for small-to-medium sequences.
    embedded_module!("diff", "std/diff.blsp"),
    // Path string manipulation: join, split, basename, dirname, extension, stem,
    // normalize, relative-to. Consolidates the prelude's path-* globals under
    // a single path/ namespace with additional operations.
    embedded_module!("path", "std/path.blsp"),
    // OS/process interface: env vars, argv, subprocess execution, OS type, halt.
    // Wraps the %env-all/%argv/%os-cmd/%os-type/%halt primitives with a clean API.
    embedded_module!("system", "std/system.blsp"),
    // Authenticated encryption (ChaCha20-Poly1305), PBKDF2 key derivation, secure
    // random bytes. Wraps the %chacha20-* and %pbkdf2-sha256-bytes primitives.
    embedded_module!("crypto", "std/crypto.blsp"),
    // The editor framework's buffer model (M2 Phase 1, ADR-045): an immutable
    // buffer over the rope primitives, opt-in, never in the prelude.
    embedded_module!("editor/buffer", "std/editor/buffer.blsp"),
    // The CLIENT half of the buffer-process protocol (ADR-134): the link record
    // + the pure push fold (echo suppression, splice transform over in-flight
    // edits, resync fallback) a subscriber uses to track a hosted document.
    embedded_module!("editor/buffer-client", "std/editor/buffer-client.blsp"),
    // The display/input seam (M3, ADR-046): `display` is the render-op protocol
    // (pure data constructors); `keymap` is the rebindable key→command dispatcher
    // shared by the line editor and the observer; `observer` is a process-viewer
    // built on them + the `term-*`/`gui-*` primitives. All opt-in, never in the prelude.
    // The shared named-face / theme registry (the counterpart to `keymap`): style
    // named once, referenced everywhere, restyled in one place. Required by `ui`
    // (so every ui-run app gets it) and the observer.
    embedded_module!("editor/face", "std/editor/face.blsp"),
    embedded_module!("editor/display", "std/editor/display.blsp"),
    embedded_module!("editor/keymap", "std/editor/keymap.blsp"),
    // Composable, runtime-reconfigurable behaviour layers over `keymap` (the
    // generic mechanism the editor's "modes" are built from; buffer-agnostic).
    // Opt-in, never in the prelude. See docs/layers.md.
    embedded_module!("editor/layers", "std/editor/layers.blsp"),
    // Structural (s-expression) navigation over the parse-source CST — reusable
    // Brood-code tooling (same tier as the formatter / LSP), not editor-specific.
    // (The text-mode/brood-mode *layers* built on it are editor policy and live in
    // the editor app — examples/editor/src/ — not here.) Opt-in. (docs/layers.md)
    embedded_module!("sexp", "std/tool/sexp.blsp"),
    // A small backtracking regular-expression engine, pure Brood (literals, ., * + ?,
    // ^ $, [...] sets, \d \w \s, |, groups; no ranges/captures yet). Opt-in.
    embedded_module!("regex", "std/regex.blsp"),
    // ANSI / VT100 escape-sequence stripping for pipe output (CSI sequences + CR).
    // Used by bshell and compile to clean subprocess output before display.
    embedded_module!("ansi", "std/ansi.blsp"),
    embedded_module!("editor/ui", "std/editor/ui.blsp"),
    // Serve a `ui-run` app to remote frontends — the Emacs `--daemon`/`emacsclient`
    // model (ADR-090): the app runs on the daemon, a thin `attach` client paints
    // pushed frames + ships back keys. Pure Brood over `ui-run` + the node link.
    embedded_module!("editor/serve", "std/editor/serve.blsp"),
    // Emacs-style tiled window splits: an immutable binary layout tree + pure
    // pane/divider geometry + drag-to-resize over `:drag` mouse events (ADR-077).
    // Reusable editor toolkit (content-agnostic); the keybindings + payload are
    // editor policy. Opt-in, never in the prelude.
    embedded_module!("editor/pane", "std/editor/pane.blsp"),
    // FORM BUFFERS (ADR-199): generated text with editable regions in it — a shell's
    // input line, a commit message's help block, a tutorial's code boxes, a rebase
    // todo. Region algebra + `splice` (re-render, keep what the user typed) + the two
    // `:post-key` guard policies (`:veto` / `:clamp`). Pure over text; opt-in.
    embedded_module!("editor/formbuf", "std/editor/formbuf.blsp"),
    // Bare ANSI escape *strings* for simple terminal scripts (`print` them
    // directly) — the lightweight counterpart to the `display` render-op
    // protocol. Opt-in, never in the prelude.
    embedded_module!("editor/ansi", "std/editor/ansi.blsp"),
    // Sets as a library over maps (ADR-062): a set is a map of `element → true`,
    // so membership/elements/size reuse `contains?`/`keys`/`count`; the module
    // adds `set`/`conj`/`disj`/`union`/`intersection`/`difference`/`subset?`.
    // Opt-in, never in the prelude (no `#{…}` literal / distinct type yet).
    embedded_module!("set", "std/set.blsp"),
    // Semantic versions as data: parse / order / test against a `">= 1.2"`,
    // `"^1.2"`, `"~> 1.3"`, or `">= 1.2, < 2.0"` constraint. Written because two
    // consumers (the registry deciding which release is newest, an application
    // deciding whether a plugin's declared `:enhances` constraint is met) had each
    // hand-rolled it. Pure predicates; the version SELECTION algorithm is `resolver`.
    embedded_module!("version", "std/version.blsp"),
    // The dependency version resolver (ADR-209): a pure backtracking, newest-compatible
    // solver over an injected `provider` (what versions exist, what each requires). The
    // registry provider that fetches for real lives in `std/tool/package`; keeping the
    // search pure here is what makes it exhaustively testable offline.
    embedded_module!("resolver", "std/resolver.blsp"),
    // Behaviour contracts — `defbehaviour` declares the ops a MODULE must define to
    // satisfy a named contract (`(:implements B)`), verified by the checker/LSP pass
    // (`types/check/protocol.rs`). Value dispatch (`defprotocol`/`defimpl`) was RETIRED
    // in favour of `ability`; behaviours stay here (a module-as-implementor contract is
    // not value dispatch). Opt-in, never in the prelude.
    embedded_module!("protocol", "std/protocol.blsp"),
    // Unified generic functions with NOMINAL dispatch (the value-polymorphism successor):
    // `defability` declares ops, `impl` registers per-identity impls from anywhere, and
    // dispatch is on the first argument's identity — its `type-of` kind, or a record's
    // The interactive REPL line editor (ADR-052): `highlight` is the pure lexical
    // syntax-highlighter / bracket-matcher / signature + completion scanners;
    // `lineedit` is the raw-mode, emacs-style editor built on it + the inline
    // `term-*` seam. Both opt-in, never in the prelude; `repl` requires them.
    // `highlight`/`lineedit` stay in CORE: they are reusable UI a shipped app may
    // `require` (the editor's minibuffer reuses `std/lineedit`'s core), not just
    // REPL plumbing — so a lean release keeps them.
    embedded_module!("editor/highlight", "std/editor/highlight.blsp"),
    // Generic tree-sitter language services (`fontify` + structural motions) over
    // the `tree-sitter-parse` builtin's positioned CST — the foreign-language
    // analogue of `sexp`+`highlight`. Pure UI a shipped editor `require`s for its
    // ruby/elixir/… modes (ROADMAP §C), so it stays in CORE; opt-in, never prelude.
    embedded_module!("editor/treesit", "std/editor/treesit.blsp"),
    // Lexical Markdown highlighter — the `highlight` analogue for `.md` buffers
    // (`markdown-spans` → `[start end face]` spans, ADR-092). Pure UI a shipped app
    // may `require` (bedit's markdown-mode), so it stays in CORE alongside
    // `highlight`/`lineedit`; opt-in, never in the prelude.
    embedded_module!("editor/markdown", "std/editor/markdown.blsp"),
    // Lexical `.env` and Dockerfile highlighters, the dotenv/Dockerfile analogues of
    // `markdown` (`env-spans` / `dockerfile-spans` → `[start end face]` spans). Pure
    // UI a shipped app may `require` (bedit's env-/docker-mode); CORE, like markdown.
    embedded_module!("editor/dotenv", "std/editor/dotenv.blsp"),
    embedded_module!("editor/dockerfile", "std/editor/dockerfile.blsp"),
    embedded_module!("editor/lineedit", "std/editor/lineedit.blsp"),
    embedded_module!("format", "std/format.blsp"),
    // The process-native tracing debugger — `break` (park without timeout),
    // `span`/`span-spawn` (cross-process causal tree), `spy` routed to a debugger
    // process. The actor-model answer to Elixir's `dbg`.
    //
    // CORE, not DEV, and this is the line the split turns on: a dev module is one that
    // serves *developing* an app (the test framework, `nest doc`, the hot-reload
    // watcher), not one an app's own shipped features are built from. A shipped editor
    // IS a debugger (bedit's `C-c d` session, its *Spy* trace stream), so a lean
    // release that omitted this couldn't run it — `require` fails at boot, since
    // `run-bundle` loads every bundled module.
    embedded_module!("debug", "std/tool/debug.blsp"),
    // A persistent, image-isolated evaluator: a dedicated child runtime runs
    // `(eval-server-run)` — one `pr-str`ed request map per stdin line, one reply
    // line back — so a parent (an editor playground, a remote REPL) gets
    // REPL-grade eval with per-request timeouts without exposing its own global
    // table. The pure codec half is shared by clients (ADR-198).
    //
    // CORE for the same reason as `debug` (which it requires): "evaluate this snippet"
    // is a shipped app's feature — bedit's tutorial playgrounds and `C-x C-e` ride
    // this codec — not a tool for building one.
    embedded_module!("eval-server", "std/tool/eval-server.blsp"),
];

/// Dev/tooling modules — baked in only under the `dev-tools` feature (the dev
/// `brood`/`nest` + tests). A `nest release` lean runtime
/// (`--no-default-features`) omits them, so a shipped app carries no test
/// framework, process observer, MCP/doc/hot-reload tooling, or interactive REPL
/// (ADR-038, docs/release.md). `project` stays in CORE — it boots the bundle;
/// `lineedit`/`highlight` stay too (reusable UI, e.g. the editor's minibuffer).
///
/// **The test for this list:** a module belongs here only if it serves *developing*
/// an app. If a shipped app's own features are built on it, it belongs in
/// [`CORE_MODULES`] however tool-shaped it looks — `debug` and `eval-server` live
/// there for exactly that reason (an editor ships a debugger and an eval
/// playground). Getting this wrong is not a graceful degradation: `run-bundle`
/// eagerly loads every bundled module, so one app module with a top-level
/// `(require 'missing)` makes the released binary fail to boot at all.
#[cfg(feature = "dev-tools")]
const DEV_MODULES: &[EmbeddedModule] = &[
    // The test framework — `deftest`/`describe`/`assert=`/`is`. Never shipped.
    embedded_module!("test", "std/tool/test.blsp"),
    // Doc generation (`nest doc`) — tooling, not runtime.
    embedded_module!("docs", "std/tool/docs.blsp"),
    // Generate editor syntax grammars (VS Code TextMate, Emacs font-lock) from the
    // language's own `(special-forms)` — one source of truth, no drift (ADR-092).
    embedded_module!("grammar", "std/tool/grammar.blsp"),
    // The process viewer / debug tooling (`nest observe`, `(observe)`).
    embedded_module!("observer", "std/tool/observer.blsp"),
    // The hot-reload file watcher — a dev-loop convenience.
    embedded_module!("reload", "std/tool/reload.blsp"),
    // The Model Context Protocol tool surface — `(mcp-tools)` returns the
    // catalogue the `nest mcp` dispatcher reads (ADR-036, docs/mcp.md, step 3).
    embedded_module!("mcp", "std/tool/mcp.blsp"),
    // The read-eval-print loop itself, written in Brood (`(require 'repl)`):
    // policy over the `read-line`/`eval-string`/`pr-str` primitives. The Rust
    // binaries (`brood`, `nest repl`) just bootstrap into `(repl-run)`. A shipped
    // app runs its own `:main`, never the REPL.
    embedded_module!("repl", "std/tool/repl.blsp"),
];

/// Empty in a lean (`--no-default-features`) release runtime — the dev modules
/// above are not compiled in at all (their `include_str!` never runs).
#[cfg(not(feature = "dev-tools"))]
const DEV_MODULES: &[EmbeddedModule] = &[];

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

/// The baked-in module registered under `key`, core table first then dev/tooling
/// (absent in a lean release runtime).
fn embedded_module(key: &str) -> Option<&'static EmbeddedModule> {
    CORE_MODULES
        .iter()
        .chain(DEV_MODULES.iter())
        .find(|m| m.key == key)
}

/// `(%builtin-module name)` — the source of a baked-in std module as a string,
/// or nil if there is none. Mechanism only: `require` (Brood) consults this
/// before searching the load-path.
pub(super) fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let Some(name) = embedded_name(heap, v) else {
        return Err(LispError::wrong_type(
            heap,
            "%builtin-module",
            "module name",
            v,
        ));
    };
    if let Some(module) = embedded_module(&name) {
        return Ok(heap.alloc_string(module.source));
    }
    // Not a baked-in std module — consult a mounted release bundle (the app's
    // own modules + bundled deps), so `require` resolves them with no change to
    // its load-path logic (ADR-038).
    match crate::bundle::mounted() {
        Some(b) => match b.module_src(&name) {
            Some(src) => Ok(heap.alloc_string(src)),
            None => Ok(Value::nil()),
        },
        None => Ok(Value::nil()),
    }
}

/// `(%builtin-module-file name)` — where a baked-in module's source was written: its
/// repo-relative path (`"std/tool/test.blsp"`), or `"<bundle>/<name>.blsp"` for a module
/// served out of a mounted release bundle, which genuinely has no path. Nil if `name`
/// isn't an embedded module at all (a load-path file has its own real path).
///
/// `require--force` hands this to `%load-string` so the module's forms are attributed to
/// the file they were written in. Without it they took the requiring file's name — see
/// [`EmbeddedModule`].
pub(super) fn builtin_module_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let Some(name) = embedded_name(heap, v) else {
        return Err(LispError::wrong_type(
            heap,
            "%builtin-module-file",
            "module name",
            v,
        ));
    };
    if let Some(module) = embedded_module(&name) {
        return Ok(heap.alloc_string(module.path));
    }
    // A bundled module has a name but no path. Say so rather than inventing one that
    // looks openable, and rather than falling back to the requiring file.
    let bundled = crate::bundle::mounted()
        .as_ref()
        .is_some_and(|b| b.module_src(&name).is_some());
    if bundled {
        let marker = format!("<bundle>/{name}.blsp");
        return Ok(heap.alloc_string(&marker));
    }
    Ok(Value::nil())
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

/// `(builtin-modules)` — the names of every module baked into this binary, as a
/// sorted list of strings. The module table is a Rust static, so the language has
/// no other way to see it; `std/tool/complete.blsp` uses it to offer `nest doc`
/// candidates, and it is generally useful for validating a module name before
/// `require`ing it.
pub(super) fn builtin_modules(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut names: Vec<&str> = CORE_MODULES
        .iter()
        .chain(DEV_MODULES.iter())
        .map(|module| module.key)
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
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

/// `(process-flag flag [value])` — read or set a per-process runtime flag on the
/// **current** process (the Erlang `process_flag/2` shape), returning the
/// previous (read: current) value. Flags:
///
/// - `:max-heap` — this process's heap limit in bytes (BEAM `max_heap_size`).
///   With a positive int: set it; with `nil`: clear it; with no value: read it.
///   Checked after each collection against the *live* (post-GC) footprint; when
///   exceeded, the next safepoint raises a catchable `E0045` error in this
///   process only — uncaught, it kills just the offender, unlike the global
///   ADR-043 hard cap (whole-OS-process abort). Policy lives in Brood: a spawn
///   wrapper that sets the limit first is `(spawn (fn () (process-flag
///   :max-heap n) (work)))`.
/// `(hibernate)` — tell the runtime this process is going idle for a long time, so it
/// should give back everything it can: collect, shrink its heap slabs and root vectors,
/// and drop its inline caches and compiled-body cache. Erlang's `erlang:hibernate/3`,
/// minus the continuation argument (Brood processes park in `receive`, so there is no
/// need to re-enter via an explicit MFA).
///
/// Returns the bytes of slab capacity released.
pub(super) fn hibernate_proc(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::Int(heap.hibernate() as i64))
}

pub(super) fn process_flag(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let flag = match arg(args, 0) {
        Value::Keyword(k) => k,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "process-flag",
                "keyword",
                other,
            ))
        }
    };
    match value::symbol_name_ref(flag) {
        "max-heap" => {
            let prev = if args.len() < 2 {
                heap.proc_mem_limit()
            } else {
                match arg(args, 1) {
                    Value::Int(n) if n > 0 => heap.set_proc_mem_limit(Some(n as usize)),
                    Value::Nil => heap.set_proc_mem_limit(None),
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "process-flag :max-heap",
                            "positive int (bytes) or nil",
                            other,
                        ))
                    }
                }
            };
            Ok(prev.map(|n| Value::int(n as i64)).unwrap_or(Value::nil()))
        }
        "send-errors" => {
            let prev = if args.len() < 2 {
                heap.proc_send_errors()
            } else {
                let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
                heap.set_proc_send_errors(on)
            };
            Ok(Value::boolean(prev))
        }
        other => Err(LispError::runtime(format!(
            "process-flag: unknown flag :{other} (known: :max-heap, :send-errors)"
        ))),
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

/// `(%receive matcher timeout tags)` — the selective-receive primitive the `receive`
/// macro (`std/prelude.blsp`) expands to. See `crate::process::receive_match`.
pub(super) fn receive_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_match(heap, arg(args, 0), arg(args, 1), arg(args, 2), arg(args, 3))
}

pub(super) fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(crate::process::pid_value(crate::process::self_pid()))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
pub(super) fn make_ref(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = crate::process::next_ref();
    // Stamp the receive-mark (ADR-195): a `receive` pinned on this ref can skip every
    // message already in our mailbox, since none of them can carry a ref that did not
    // exist when they were enqueued. One relaxed atomic load, no mailbox lock.
    heap.set_recv_mark(id, crate::process::self_mailbox_seq());
    Ok(Value::ref_(id))
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
    let path = expect_string(heap, "spit-private", arg(args, 0))?;
    let content = expect_string(heap, "spit-private", arg(args, 1))?;
    let err = |e: std::io::Error| {
        LispError::runtime(format!("spit-private: {path}: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    // Owner-only 0600 permissions are a Unix concept; wasm has no filesystem perms,
    // so it falls back to a plain private-intent write.
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::File::create(&path).map_err(err)?;
        f.write_all(content.as_bytes()).map_err(err)?;
    }
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

/// `(features)` — the optional build features this runtime was compiled with, as a
/// vector of keywords (e.g. `[:jit :treesit :gui]`).
///
/// The point is that a *bound* builtin does not imply a working one: with the `gui`
/// feature off, `gui-open` is still bound and still raises at call time, so
/// `(bound? 'gui-open)` answers "yes" on a runtime that cannot open a window. An
/// app that wants to degrade rather than fail needs to ask the build, not the
/// environment — and the only alternative was provoking the error and matching on
/// its prose, which silently breaks whenever the message is reworded.
pub(super) fn features(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Order is stable (declaration order, not cfg order) so a printed value diffs
    // cleanly between builds.
    let mut out = Vec::new();
    if cfg!(feature = "gui") {
        out.push(value::kw("gui"));
    }
    if cfg!(feature = "gui-gpu") {
        out.push(value::kw("gui-gpu"));
    }
    if cfg!(feature = "audio") {
        out.push(value::kw("audio"));
    }
    if cfg!(feature = "clipboard") {
        out.push(value::kw("clipboard"));
    }
    if cfg!(feature = "jit") {
        out.push(value::kw("jit"));
    }
    if cfg!(feature = "treesit") {
        out.push(value::kw("treesit"));
    }
    if cfg!(feature = "wasm") {
        out.push(value::kw("wasm"));
    }
    if cfg!(feature = "dev-tools") {
        out.push(value::kw("dev-tools"));
    }
    if cfg!(feature = "perf-stats") {
        out.push(value::kw("perf-stats"));
    }
    Ok(heap.alloc_vector(out))
}

/// `(worker-threads)` — size of the scheduler's worker-thread pool that runs the
/// green processes (≈ `nproc`, or the `-j` setting); 0 until the first spawn.
pub(super) fn worker_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::worker_threads() as i64))
}

/// `(sched-stats)` — one snapshot map of the scheduler's cumulative counters
/// (the scheduler half of the observability timing tier): `:spawned`/`:exited`
/// totals (their difference is the live-process figure), `:preempts` (quantum
/// exhaustions), `:steals` + `:migrations` (work-stealing activity),
/// `:workers` and `:peak-threads` (pool size / high-water parallelism).
pub(super) fn sched_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pairs = vec![
        (
            value::kw("spawned"),
            Value::int(crate::process::spawn_count() as i64),
        ),
        (
            value::kw("exited"),
            Value::int(crate::process::exit_count() as i64),
        ),
        (
            value::kw("preempts"),
            Value::int(crate::process::preempt_count() as i64),
        ),
        (
            value::kw("steals"),
            Value::int(crate::process::steal_count() as i64),
        ),
        (
            value::kw("migrations"),
            Value::int(crate::process::migrate_count() as i64),
        ),
        (
            value::kw("workers"),
            Value::int(crate::process::worker_threads() as i64),
        ),
        (
            value::kw("peak-threads"),
            Value::int(crate::process::peak_threads() as i64),
        ),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(profile-start [hz])` — arm the sampling CPU profiler at `hz` samples/sec
/// (default 99, clamped 1..10000). Resets the histogram; see `profile-stop`.
pub(super) fn profile_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let hz = match arg(args, 0) {
        Value::Nil => 99,
        Value::Int(n) if n > 0 => n.min(10_000) as u32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "profile-start",
                "positive int (hz) or absent",
                other,
            ))
        }
    };
    crate::profile::start(hz);
    Ok(Value::nil())
}

/// `(profile-stop)` — disarm the sampling profiler and return the histogram: a
/// list of `{:stack (fn-names… innermost-first) :count n}` maps, most-sampled
/// first. A sample whose frames were all anonymous appears with `:stack
/// ("<anonymous>")`.
pub(super) fn profile_stop(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let entries = crate::profile::stop();
    let items: Vec<Value> = entries
        .iter()
        .map(|(stack, count)| {
            let names: Vec<Value> = if stack.is_empty() {
                vec![heap.alloc_string("<anonymous>")]
            } else {
                stack
                    .iter()
                    .map(|&s| heap.alloc_string(value::symbol_name_ref(s)))
                    .collect()
            };
            let stack_list = heap.list(names);
            let pairs = vec![
                (value::kw("stack"), stack_list),
                (value::kw("count"), Value::int(*count as i64)),
            ];
            heap.map_from_pairs(pairs)
        })
        .collect();
    Ok(heap.list(items))
}

/// The `(system-monitor)` return shape: the armed config as a map, or nil.
fn sysmon_config_map(heap: &mut Heap, m: Option<crate::process::sysmon::SysMon>) -> Value {
    match m {
        None => Value::nil(),
        Some(m) => {
            let pairs = vec![
                (value::kw("pid"), crate::process::pid_value(m.pid)),
                (value::kw("gc"), Value::boolean(m.gc)),
                (
                    value::kw("gc-min-pause-us"),
                    Value::int(m.gc_min_pause_us as i64),
                ),
                (value::kw("spawn"), Value::boolean(m.spawn)),
                (value::kw("exit"), Value::boolean(m.exit)),
                (value::kw("deopt"), Value::boolean(m.deopt)),
            ];
            heap.map_from_pairs(pairs)
        }
    }
}

/// `(system-monitor [pid opts])` — read, arm, or clear the kernel **system
/// monitor**: runtime events (`:gc`/`:spawn`/`:exit`/`:deopt`) delivered to one
/// subscriber process as `[:system kind subject-pid detail]` messages (BEAM
/// `system_monitor` shape; see `process/sysmon.rs`). No args reads the current
/// config; `nil` clears; a local pid arms it — with no opts map every event is
/// selected, with one exactly the truthy keys are. Arming/clearing returns the
/// *previous* config (map or nil), so callers can save/restore.
pub(super) fn system_monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::process::sysmon::{self, SysMon};
    if args.is_empty() {
        return Ok(sysmon_config_map(heap, sysmon::current()));
    }
    let prev = match arg(args, 0) {
        Value::Nil => sysmon::install(None),
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            let mut m = SysMon {
                pid: id,
                gc: true,
                gc_min_pause_us: 0,
                spawn: true,
                exit: true,
                deopt: true,
            };
            if args.len() > 1 {
                match arg(args, 1) {
                    // An explicit opts map selects exactly its truthy keys.
                    Value::Map(opts) => {
                        let sel = |heap: &Heap, name: &str| {
                            heap.map_get(opts, value::kw(name))
                                .is_some_and(crate::eval::truthy)
                        };
                        m.gc = sel(heap, "gc");
                        m.spawn = sel(heap, "spawn");
                        m.exit = sel(heap, "exit");
                        m.deopt = sel(heap, "deopt");
                        if let Some(Value::Int(n)) =
                            heap.map_get(opts, value::kw("gc-min-pause-us"))
                        {
                            if n > 0 {
                                m.gc_min_pause_us = n as u64;
                            }
                        }
                    }
                    Value::Nil => {}
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "system-monitor",
                            "options map or nil",
                            other,
                        ))
                    }
                }
            }
            sysmon::install(Some(m))
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "system-monitor",
                "local pid or nil",
                other,
            ))
        }
    };
    Ok(sysmon_config_map(heap, prev))
}

/// `(build-id)` — this `brood` build's identity, `"<version>+<git-sha>+<binary-
/// stamp>"` (e.g. `"0.1.0+dcab7ca+18f2e1a9b3c4d5e6"`). The correct staleness
/// stamp for an on-disk cache of anything the kernel computes (the checker's
/// own logic is Rust, so its results are not portable across binaries).
///
/// The git-sha half is baked in at compile time (`BROOD_GIT_SHA`) and is
/// **not** by itself a reliable staleness stamp: it's `git rev-parse --short
/// HEAD`, which doesn't change across an uncommitted rebuild on the same
/// commit (exactly the case during active development on the checker
/// itself), and `build.rs`'s `rerun-if-changed` only watches `.git/HEAD`/
/// `.git/refs/heads` — a plain source edit + rebuild doesn't even re-run it.
/// The `binary-stamp` half (this executable's own mtime, read at *runtime*
/// via [`binary_stamp`]) closes that gap: it changes on literally any
/// rebuild, committed or not, for any reason, with no `build.rs` changes
/// needed — correct by construction rather than by tracking which source
/// files matter to which cache.
pub(super) fn build_id(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = build_id_string();
    Ok(heap.alloc_string(&id))
}

/// `(brood-version)` — this runtime's semantic version (`CARGO_PKG_VERSION`,
/// e.g. `"0.1.0"`): the string a project's `:brood` manifest constraint is
/// checked against (ADR-209). Just the semver — the git-sha and binary-stamp
/// live in `build-id`. The kernel is the only place this value exists, so it is
/// a primitive; the policy that reads a `:brood` constraint and refuses an
/// incompatible runtime is Brood (`std/tool/project.blsp`).
pub(super) fn brood_version(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_string(env!("CARGO_PKG_VERSION")))
}

/// The `(build-id)` string as plain Rust — shared with the boot cache
/// (`lib.rs`), which uses it as the staleness key for the expanded-prelude
/// cache (the prelude is `include_str!`'d, so any binary change covers it).
pub(crate) fn build_id_string() -> String {
    format!(
        "{}+{}+{}",
        env!("CARGO_PKG_VERSION"),
        env!("BROOD_GIT_SHA"),
        binary_stamp()
    )
}

/// This running executable's own last-modified time, as a hex nanosecond
/// stamp — computed once per process (`OnceLock`) since it never changes
/// mid-run. `"unknown"` if the executable path or its metadata can't be read
/// (e.g. a sandboxed environment with no `/proc/self/exe`-equivalent) — a
/// stable-but-uninformative fallback, not a crash; the git-sha half of
/// `build_id` still carries some staleness signal in that case.
fn binary_stamp() -> &'static str {
    static STAMP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STAMP.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| format!("{:x}", d.as_nanos()))
            .unwrap_or_else(|| "unknown".to_string())
    })
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
    // `snapshot_globals`/`restore_globals` now bracket RUNTIME compaction themselves (the
    // snapshot holds off-graph RUNTIME handles a mid-thunk relocation would strand — KI-6),
    // so this reset+run+rollback is compaction-safe with no extra bookkeeping here.
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
        // Never reap the CALLER: the root's mailbox registers lazily (its first
        // `receive`), so a root that had never received before this isolate ran
        // shows up as a "newcomer" — and the reap would exit-kill the very process
        // running the isolate. That kill was silently ignored for as long as
        // exit signals couldn't reach a natively-nested receive; now that they can
        // (Control::Killed), it would abort the whole run.
        .filter(|p| !before.contains(p) && *p != crate::process::self_pid())
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
/// references to `foo/…`. Returns the (possibly rooted) namespace symbol.
///
/// Under an active dependency load (ADR-070), the declared name is **rooted** to the
/// package: loading dep `foo`'s `b.blsp` — which says `(defmodule b)` → `(%in-ns 'b)`
/// — sets `compile_ns` to `foo/b`, so the file's `def`s become `foo/b/…`. Outside a
/// dep load (root project / std) the name is unchanged.
pub(super) fn in_ns(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%in-ns", arg(args, 0))?;
    let rooted = heap.root_module_name(sym);
    heap.set_compile_ns(Some(rooted));
    Ok(Value::symbol(rooted))
}

/// `(%root-module-name 'b)` — root a referenced module name to the active package:
/// `foo/b` while loading dep `foo` if `b` is one of `foo`'s modules, else `b`
/// unchanged (ADR-070). The loader emits it around `(:use …)`/`(:alias …)`/`require`
/// targets and `defmodule`'s provide/doc key so intra-package references and the
/// module's own registration all agree on the rooted global identity.
pub(super) fn root_module_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%root-module-name", arg(args, 0))?;
    Ok(Value::symbol(heap.root_module_name(sym)))
}

/// `(%set-package-context 'foo '(a b c))` — enter dep `foo`'s load with its provided
/// short module names, returning `[prev-prefix prev-modules]` (a tuple) so the caller
/// restores the enclosing context after the load (dep loads nest). `(%set-package-context
/// nil nil)` clears it. Roots every module name the load declares or references (ADR-070).
pub(super) fn set_package_context(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Nil => None,
        v => Some(expect_symbol(heap, "%set-package-context", v)?),
    };
    let modules: std::collections::HashSet<crate::core::value::Symbol> = heap
        .list_to_vec(arg(args, 1))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| match v {
            Value::Sym(s) => Some(s),
            _ => None,
        })
        .collect();
    let (prev_prefix, prev_modules) = heap.set_package_context(prefix, modules);
    let prev_mods_list: Vec<Value> = prev_modules.into_iter().map(Value::Sym).collect();
    let list = heap.list(prev_mods_list);
    let prefix_val = prev_prefix.map(Value::Sym).unwrap_or(Value::nil());
    Ok(heap.alloc_vector(vec![prefix_val, list]))
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

/// `(%mark-private 'name)` — record the global `name` as module-private (ADR-146).
/// Emitted by the `defn-`/`def-` macros alongside their `def`. `name` is qualified
/// to the current namespace *exactly as a `def` head would be* — via
/// [`resolve_reference`](crate::eval::macros::resolve_reference), the same entry
/// `%register-sig` uses — so the recorded key matches the qualified global the def
/// created (and the `def` runs first, so that global already exists). Privacy is now
/// a fact the def form declares here, not one derived from the name's spelling.
/// Returns the qualified name so it composes inside the macro's `do`.
pub(super) fn mark_private(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%mark-private", arg(args, 0))?;
    let qualified = crate::eval::macros::resolve_reference(heap, name);
    heap.mark_private(qualified);
    Ok(Value::symbol(qualified))
}

/// Add one `(:use …)` import (bare → qualified) to the current file's table,
/// enforcing the two Elixir-style import rules:
///
/// - **Clash (error).** If `bare` is already imported from a *different* module, it's
///   a hard error naming both — resolvable with `:only`/`:exclude` on one of the uses.
///   Re-importing the *same* qualified name (a re-`:use` of the same module) is a
///   no-op, so idempotent reloads are fine.
/// - **Shadow (warning).** If `bare` already names a live root/prelude/builtin global,
///   the import shadows it — allowed (the resolver gives an import precedence over
///   root), but warned, exactly as Elixir warns when an import shadows an
///   auto-imported `Kernel` name. Reach the original with the `/name` root escape, or
///   silence per-name with `:exclude`. `BROOD_NO_SHADOW_WARN` mutes the class.
fn refer_add(
    heap: &mut Heap,
    bare: value::Symbol,
    qualified: value::Symbol,
    mod_name: &str,
) -> Result<(), LispError> {
    // An ambient (`defdyn`) name always resolves bare/root — the resolver's `is_ambient`
    // check short-circuits before it ever consults the import table — so an import for
    // one is inert, and it can neither clash nor shadow. Skip it: no entry, no error, no
    // warning (a dynamic knob like `*width*` shared by two modules must not read as one).
    if value::is_dynamic(bare) {
        return Ok(());
    }
    if let Some(existing) = heap.import_of(bare) {
        if existing == qualified {
            return Ok(()); // idempotent — same module referred again (e.g. reload)
        }
        return Err(LispError::runtime(format!(
            "(:use {mod_name}) refers `{}`, but it is already referred as `{}` from another \
             module — resolve the clash with `:only [...]` or `:exclude [...]` on one of the uses",
            value::symbol_name(bare),
            value::symbol_name(existing),
        )));
    }
    if heap.env_get(value::EnvId::GLOBAL, bare).is_some()
        && std::env::var_os("BROOD_NO_SHADOW_WARN").is_none()
    {
        let b = value::symbol_name(bare);
        eprintln!(
            "warning: (:use {mod_name}) refers `{b}`, which shadows the prelude/root `{b}`; \
             reach the original as `/{b}`, or drop it with `:exclude [{b}]`"
        );
    }
    heap.add_import(bare, qualified);
    Ok(())
}

/// Is `mod_name` currently mid-load — present in the `*features-loading*` in-flight
/// table (ADR-136)? Outside a cycle this is always false at `%refer` time: a normal
/// `(:use m)` fully loads and `provide`s `m` (clearing the marker) *before* its
/// `%refer` runs. A module still loading here therefore means the current file is
/// being referred from *inside* `m`'s own load — a `(:use)` cycle, whose refer-all
/// would silently import only the names defined so far.
fn module_is_loading(heap: &mut Heap, mod_name: &str) -> bool {
    let map_id = match heap
        .env_get(value::EnvId::GLOBAL, value::intern("*features-loading*"))
        .map(|v| v.unpack())
    {
        Some(crate::core::value::ValueRef::Map(id)) => id,
        _ => return false,
    };
    let key = heap.alloc_string(mod_name);
    heap.map_get(map_id, key).is_some()
}

/// `(%refer 'mod subset exclude)` — add `(:use …)` imports to the current file's
/// import table (ADR-065 inc-2). `mod` must already be loaded (the `defmodule` macro
/// emits a `(require 'mod)` first). `subset` nil → refer every *public* `mod/name`
/// (not recorded private, not itself nested); else a seq of bare symbols → refer
/// just those as `mod/name`. `exclude` (a seq of bare names, or nil) drops those from
/// a refer-all — Elixir's `except:`. Each import becomes a bare → qualified entry the
/// resolver consults after the current namespace and before root; clashes and
/// prelude shadows are policed by [`refer_add`].
pub(super) fn refer(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mod_sym = expect_symbol(heap, "%refer", arg(args, 0))?;
    let mod_name = value::symbol_name(mod_sym);
    let prefix = format!("{}/", mod_name);
    // The `:exclude` set (bare symbols to skip in a refer-all).
    let excluded: std::collections::HashSet<value::Symbol> = match arg(args, 2) {
        Value::Nil => std::collections::HashSet::new(),
        ex => heap
            .seq_items(ex)?
            .into_iter()
            .filter_map(|v| match v {
                Value::Sym(s) => Some(s),
                _ => None,
            })
            .collect(),
    };
    match arg(args, 1) {
        Value::Nil => {
            // A refer-all against a module still mid-load is a circular `(:use …)`:
            // its public set is incomplete, so importing "all" of it would silently
            // miss the names defined after the cycle point. Fail clearly instead —
            // `:only` (lazy, resolved at reference time) is the cycle-safe escape.
            if module_is_loading(heap, &mod_name) {
                return Err(LispError::runtime(format!(
                    "circular `(:use {mod_name})`: `{mod_name}` is still loading (a cycle back \
                     into this module), so a refer-all would import only the names defined so \
                     far. Break the cycle, or import just what you need with \
                     `(:use {mod_name} :only [...])`, which resolves lazily and is cycle-safe."
                )));
            }
            // Refer all public names: enumerate the live globals under `mod/`.
            for g in heap.global_symbols() {
                let name = value::symbol_name(g);
                if let Some(bare) = name.strip_prefix(&prefix) {
                    // `g` is a live enumerated global under `mod/`, so `is_private`
                    // (the recorded fact) is exact — the module is loaded here.
                    if !bare.is_empty() && !bare.contains('/') && !heap.is_private(g) {
                        let bare_sym = value::intern(bare);
                        if excluded.contains(&bare_sym) {
                            continue;
                        }
                        refer_add(heap, bare_sym, g, &mod_name)?;
                    }
                }
            }
        }
        subset => {
            // Refer just the named symbols as `mod/name` (existence not required —
            // an unbound `mod/name` surfaces as a normal unbound-reference error).
            for item in heap.seq_items(subset)? {
                let bare = expect_symbol(heap, "%refer", item)?;
                let bare_name = value::symbol_name(bare);
                let qualified = value::intern(&format!("{}/{}", mod_name, bare_name));
                // A module-private name in an explicit :only list is a privacy breach
                // unless this file holds an internals grant for the module (ADR-146) —
                // same rule the resolver enforces for qualified references. Privacy is
                // the recorded fact (`is_private`): the module is being imported, so it
                // is loaded and the record is exact.
                if heap.is_private(qualified)
                    && heap
                        .import_of(crate::eval::macros::internals_grant_key(&mod_name))
                        .is_none()
                {
                    return Err(LispError::runtime(format!(
                        "(:use {mod_name} :only [... {bare_name} ...]): `{bare_name}` is module-private (ADR-146); grant access with (:use-internals {mod_name}) or use the public API"
                    )));
                }
                refer_add(heap, bare, qualified, &mod_name)?;
            }
        }
    }
    Ok(Value::nil())
}

/// `(%grant-internals 'mod)` — the `(:use-internals mod)` header clause's
/// mechanism (ADR-146): record that the CURRENT file may reference `mod`'s
/// module-private names (qualified access), which is otherwise a compile
/// error. Stored in the per-file import table under the impossible key
/// `/internals/<mod>` (the `%alias` trick), so it rides the same save/restore
/// lifecycle as every other import.
pub(super) fn grant_internals(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let m = expect_symbol(heap, "%grant-internals", arg(args, 0))?;
    let key = crate::eval::macros::internals_grant_key(&value::symbol_name(m));
    heap.add_import(key, m);
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

/// `(%trace-context)` — the debugger's durable per-process causal context (ADR-174),
/// or nil. A settable per-process slot (unlike a `binding`): `spawn` copies it into a
/// child, `send` ships it, `receive` overwrites it on pop. `dev-tools` only.
#[cfg(feature = "dev-tools")]
pub(super) fn trace_context_get(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.trace_context().unwrap_or(Value::Nil))
}

/// `(%set-trace-context ctx)` — set (or, with nil, clear) the per-process trace
/// context. Returns nil. `dev-tools` only.
#[cfg(feature = "dev-tools")]
pub(super) fn trace_context_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Set from Brood (`with-debugger`/`span`) → this is the process's OWN context,
    // so `spawn` propagates it. Message-adoption uses `own = false` (in the mailbox).
    heap.set_trace_context(Some(arg(args, 0)), true);
    Ok(Value::Nil)
}

// ---------- the dirty-native offload pool (ADR-144) ----------

/// Natives a green process may run on the offload pool (`%offload`):
/// long/blocking, data-in/data-out — each touches only the scratch heap it is
/// handed (no globals, no env lookups, no process identity), so running it
/// off-process is sound. Everything else is refused: offloading a
/// heap-sharing or env-reading native would race the caller's world.
const OFFLOAD_ALLOWED: &[&str] = &[
    "%git-clone",
    "%git-resolve-ref",
    "%git-list-tags",
    "%untar-gz",
    "%pbkdf2-sha256-bytes",
    "%digest",
    "%hmac",
    "%gzip",
    "%gunzip",
    "%zlib-compress",
    "%zlib-uncompress",
    "%deflate",
    "%inflate",
    "slurp",
    "slurp-bytes",
    "spit",
    "spit-bytes",
    "spit-append",
    "append-bytes",
    "tls-self-signed",
    // A long guest call is exactly what the pool is for (docs/interop.md):
    // the handle is an int token, args/results are data, and the instance
    // registry is global with a per-instance mutex — off-worker is sound.
    "%wasm-call",
];

struct OffloadJob {
    func: value::NativeFnPtr,
    args: Vec<crate::process::Message>,
    sink: crate::process::MailboxSink,
    token: i64,
}

static OFFLOAD_TOKEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// The pool: a few OS threads sharing one job queue (dirty work is the
/// exception, not the load — BEAM's dirty schedulers are similarly few).
/// Workers block on the shared receiver; the mutex is held only across the
/// dequeue, so jobs run concurrently.
fn offload_pool() -> &'static std::sync::Mutex<std::sync::mpsc::Sender<OffloadJob>> {
    use std::sync::{mpsc, Arc, Mutex, OnceLock};
    static POOL: OnceLock<Mutex<mpsc::Sender<OffloadJob>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<OffloadJob>();
        let rx = Arc::new(Mutex::new(rx));
        let n = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);
        let workers = (n / 4).max(2);
        for i in 0..workers {
            let rx = Arc::clone(&rx);
            std::thread::Builder::new()
                .name(format!("brood-offload-{i}"))
                .spawn(move || loop {
                    let job = rx.lock().expect("offload queue").recv();
                    match job {
                        Ok(j) => run_offload_job(j),
                        Err(_) => break,
                    }
                })
                .expect("spawn offload worker");
        }
        Mutex::new(tx)
    })
}

/// Run one job on a pool thread: rebuild the args in a private scratch heap,
/// call the native, ship the result (or the structured error) back as a
/// mailbox message. The scratch heap dies with the job — nothing is shared.
fn run_offload_job(job: OffloadJob) {
    let OffloadJob {
        func,
        args,
        sink,
        token,
    } = job;
    // Contain a *panic* in the native (an interpreter bug, not a Brood `Err`)
    // like the scheduler contains a panicking green process: without this, a
    // panic unwinds and kills the worker thread permanently, and — with only
    // ~nproc/4 workers — a couple of them drain the pool so every future
    // `offload` (incl. `nest fetch`'s `%git-clone`) hangs forever on its
    // `receive`. The per-job scratch heap is local to the closure, so a torn
    // heap is discarded either way. On a caught panic the caller gets a
    // structured `[:offload-error …]`, not silence.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut heap = Heap::new();
        let env = heap.new_env(None);
        let mut vals = Vec::with_capacity(args.len());
        for m in &args {
            vals.push(crate::process::from_message(&mut heap, m));
        }
        match func(&vals, env, &mut heap) {
            Ok(v) => match crate::process::to_message(&heap, v) {
                Ok(m) => offload_msg("offload", token, m),
                Err(e) => offload_msg("offload-error", token, crate::process::error_reason(&e)),
            },
            Err(e) => offload_msg("offload-error", token, crate::process::error_reason(&e)),
        }
    }));
    let msg = outcome.unwrap_or_else(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let e = LispError::runtime(format!(
            "offload: the native panicked (interpreter bug): {detail}"
        ));
        offload_msg("offload-error", token, crate::process::error_reason(&e))
    });
    sink.emit(msg);
}

fn offload_msg(tag: &str, token: i64, payload: crate::process::Message) -> crate::process::Message {
    use crate::process::Message;
    Message::Vector(vec![
        Message::Keyword(value::intern(tag)),
        Message::Int(token),
        payload,
    ])
}

/// `(%offload f args)` — run the allowed blocking native `f` with `args` (a
/// vector) on the offload pool. Returns a token int at once; the pool later
/// delivers `[:offload token result]` or `[:offload-error token err]` to the
/// calling process's mailbox. Policy lives in the prelude `offload` wrapper.
pub(super) fn offload_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (func, name) = match arg(args, 0) {
        Value::Native(id) => {
            let n = heap.native(id);
            (n.func, n.name.clone())
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%offload",
                "native function",
                other,
            ))
        }
    };
    if !OFFLOAD_ALLOWED.contains(&name.as_str()) {
        return Err(LispError::runtime(format!(
            "%offload: `{name}` is not offload-safe — only long/blocking data-in/data-out natives run on the pool (see (doc '%offload))"
        )));
    }
    let call_args: Vec<Value> = match arg(args, 1) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil => Vec::new(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%offload",
                "vector of arguments",
                other,
            ))
        }
    };
    let mut msgs = Vec::with_capacity(call_args.len());
    for v in call_args {
        msgs.push(crate::process::to_message(heap, v)?);
    }
    let token = OFFLOAD_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (sink, _cell) = crate::process::sink_pair(crate::process::self_pid());
    let job = OffloadJob {
        func,
        args: msgs,
        sink,
        token,
    };
    let _ = offload_pool().lock().expect("offload queue").send(job);
    Ok(Value::Int(token))
}

// ---------- WASM component interop (ADR-071/145, feature `wasm`) ----------

/// `(%wasm-load content)` — instantiate a sandboxed WASM component from
/// `content`: a `bytes` value (a compiled `.wasm` component) or a string (WAT
/// text — handy for tests and the REPL). Returns the instance token.
#[cfg(feature = "wasm")]
pub(super) fn wasm_load(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let bytes: Vec<u8> = match arg(args, 0) {
        Value::Bytes(id) => heap.bytes(id).as_bytes().to_vec(),
        Value::Str(id) => heap.string(id).as_bytes().to_vec(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%wasm-load",
                "bytes (a compiled .wasm component) or string (WAT source)",
                other,
            ))
        }
    };
    crate::wasm::load(&bytes).map(|id| Value::Int(id as i64))
}

/// `(%wasm-call inst name args)` — call export `name` of instance `inst` with
/// `args` (a vector), marshalled by the export's WIT types. Fuel-metered.
#[cfg(feature = "wasm")]
pub(super) fn wasm_call(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-call", arg(args, 0))? as u64;
    let name = expect_string(heap, "%wasm-call", arg(args, 1))?.to_string();
    let call_args: Vec<Value> = match arg(args, 2) {
        Value::Vector(vid) => heap.vector(vid).to_vec(),
        Value::Nil => Vec::new(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%wasm-call",
                "vector of arguments",
                other,
            ))
        }
    };
    crate::wasm::call(heap, id, &name, &call_args)
}

/// `(%wasm-exports inst)` — the instance's exported functions, as a vector of
/// `[name arity]` pairs (sorted by name).
#[cfg(feature = "wasm")]
pub(super) fn wasm_exports(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-exports", arg(args, 0))? as u64;
    let entries = crate::wasm::exports(id)?;
    let mut out = Vec::with_capacity(entries.len());
    for (name, arity) in entries {
        let n = heap.alloc_string(&name);
        out.push(heap.alloc_vector(vec![n, Value::Int(arity as i64)]));
    }
    Ok(heap.alloc_vector(out))
}

/// `(%wasm-close inst)` — drop the instance (idempotent); the sandbox and
/// everything the guest owns is freed. Returns nil.
#[cfg(feature = "wasm")]
pub(super) fn wasm_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_int(heap, "%wasm-close", arg(args, 0))? as u64;
    crate::wasm::close(id);
    Ok(Value::nil())
}

/// `[file (line …)]` pairs, the shape both coverage readouts return.
fn coverage_pairs(entries: Vec<(String, Vec<u32>)>, heap: &mut Heap) -> LispResult {
    let mut out = Vec::new();
    for (file, lines) in entries {
        let file_val = heap.alloc_string(&file);
        let line_vals: Vec<Value> = lines.iter().map(|l| Value::int(i64::from(*l))).collect();
        let lines_val = heap.list(line_vals);
        out.push(heap.alloc_vector2(file_val, lines_val));
    }
    Ok(heap.list(out))
}

/// `(%coverage-lines)` — every line recorded as EXECUTED, as a list of
/// `[file (line …)]`. Empty unless the run was started with `BROOD_COVERAGE=1`
/// (which `nest test --cover-lines` sets before building the prelude).
pub(super) fn coverage_lines(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    coverage_pairs(crate::coverage::snapshot(), heap)
}

/// `(%coverage-instrumented)` — every line the compiler INSTRUMENTED, same shape. The
/// denominator for a percentage: without it the two halves of the ratio would come
/// from different populations (see `coverage.rs`). A never-called function is present
/// here and absent from `%coverage-lines` — provided it was forced through
/// `%coverage-precompile` first, since arms otherwise compile on first call.
pub(super) fn coverage_instrumented(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    coverage_pairs(crate::coverage::instrumented(), heap)
}

/// `[file ([line col taken] …)]` pairs — the shape the branch-hit readout returns.
fn coverage_branch_pairs(
    entries: Vec<(String, Vec<(u32, u32, bool)>)>,
    heap: &mut Heap,
) -> LispResult {
    let mut out = Vec::new();
    for (file, edges) in entries {
        let file_val = heap.alloc_string(&file);
        let edge_vals: Vec<Value> = edges
            .iter()
            .map(|(line, col, taken)| {
                let items = vec![
                    Value::int(i64::from(*line)),
                    Value::int(i64::from(*col)),
                    Value::boolean(*taken),
                ];
                heap.alloc_vector(items)
            })
            .collect();
        let edges_val = heap.list(edge_vals);
        out.push(heap.alloc_vector2(file_val, edges_val));
    }
    Ok(heap.list(out))
}

/// `(%coverage-branches)` — every branch edge recorded as taken, as
/// `[file ([line col taken] …)]`. A branch is fully covered when both `taken` edges
/// (`true` and `false`) appear for one `[line col]`. Empty unless `BROOD_COVERAGE=1`.
pub(super) fn coverage_branches(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    coverage_branch_pairs(crate::coverage::branch_snapshot(), heap)
}

/// `(%coverage-branch-instrumented)` — every `[line col]` decision point the compiler
/// instrumented, as `[file ([line col] …)]` — the branch denominator (two edges each).
pub(super) fn coverage_branch_instrumented(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let entries = crate::coverage::branch_instrumented();
    let mut out = Vec::new();
    for (file, sites) in entries {
        let file_val = heap.alloc_string(&file);
        let site_vals: Vec<Value> = sites
            .iter()
            .map(|(line, col)| {
                heap.alloc_vector(vec![
                    Value::int(i64::from(*line)),
                    Value::int(i64::from(*col)),
                ])
            })
            .collect();
        let sites_val = heap.list(site_vals);
        out.push(heap.alloc_vector2(file_val, sites_val));
    }
    Ok(heap.list(out))
}

/// `(%coverage-precompile f)` — compile `f`'s body now, without calling it, so its
/// lines land in `%coverage-instrumented`. Returns true if a body was compiled.
/// See `eval::compile::precompile` for why the denominator needs this.
pub(super) fn coverage_precompile(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(crate::eval::compile::precompile(
        heap, args[0],
    )))
}

/// `(%coverage-reset)` — forget every recorded line, so a long-lived image can
/// measure more than once without runs bleeding together.
pub(super) fn coverage_reset(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    crate::coverage::reset();
    Ok(Value::nil())
}

#[cfg(test)]
mod tests {
    use super::{CORE_MODULES, DEV_MODULES};

    /// A release bundle runs on the LEAN runtime, which compiles [`DEV_MODULES`] away
    /// entirely — and `run-bundle` loads every module the app ships, so one top-level
    /// `(require 'x)` for a dev-only `x` means the released binary cannot boot at all.
    /// These two are the capabilities a shipped app's own features are built from (an
    /// editor ships a debugger and an eval playground), so they must stay in CORE.
    /// This test is the guard: moving either back to DEV breaks `nest release`, and
    /// the symptom is a failure to start, far from the cause.
    #[test]
    fn app_runtime_capabilities_stay_out_of_dev_modules() {
        for key in ["debug", "eval-server"] {
            assert!(
                CORE_MODULES.iter().any(|m| m.key == key),
                "`{key}` must be in CORE_MODULES — a lean release runtime omits DEV_MODULES, \
                 so an app requiring it would fail to boot"
            );
            assert!(
                !DEV_MODULES.iter().any(|m| m.key == key),
                "`{key}` is in DEV_MODULES; it is a shipped-app capability, not dev tooling"
            );
        }
    }

    /// Every baked-in module is reachable under exactly one key, in one list. A stem
    /// listed twice (say `debug` left in DEV while also added to CORE) would resolve by
    /// whichever list `embedded_module` scans first — a silent split-brain.
    #[test]
    fn embedded_module_keys_are_unique() {
        let mut keys: Vec<&str> = CORE_MODULES
            .iter()
            .chain(DEV_MODULES.iter())
            .map(|m| m.key)
            .collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "a baked-in module key is listed twice");
    }
}
