//! Brood source as **data**: the reader primitives (`read-string`, `%read-all`), the
//! positioned CST readers the LSP and formatter consume, and the definition scanner
//! (`%scan-source-extract`) that lists what a file defines without evaluating it.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::LispResult;
use crate::syntax::{cst, reader};

use super::numeric::{arg, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%read-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["s"],
        "Parse and return the single form in string s. Errors on trailing content after the form (rather than silently dropping it) — use reflect/read-all for input with more than one form.",
        read_string);
    primitives.def(
        "%read-all",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["s"],
        "Parse every form in string s and return them as a list (the all-forms sibling of reflect/read-string).",
        read_all);
    primitives.def(
        "%read-first",
        Arity::exact(1),
        Sig::new(vec![string], any),
        &["s"],
        "Parse and return the first form in string s, ignoring any trailing forms (the lenient sibling of reflect/read-string — for peeking a multi-form source's leading form, e.g. a file's (defmodule …) header).",
        read_first);
    // CST parse — mechanism for the in-Brood formatter (std/format.blsp); never
    // fails (malformed input becomes [:error "..."] nodes). Returns nested
    // vectors; see `parse_source` for the shape.
    primitives.def(
        "%parse-source",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        &["s"],
        "Parse s into a lossless CST tree as nested vectors (mechanism for std/format.blsp).",
        parse_source,
    );
    primitives.def(
        "%scan-source-extract",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        &["src"],
        "Native per-file scan for the whole-project check (ADR-119): parse src and return [counts privs def-names] — a map of every symbol's occurrence count, this file's `defn-`/`def-` privates as [bare qual], and every top-level def's qualified name. The fast path replacing the interpreted CST walk.",
        scan_source_extract);
    // CST parse with absolute positions — every node a map `{:kind :start :end …}`
    // (char offsets). Backs structural navigation (std/sexp); see
    // `parse_source_positioned` for the shape.
    primitives.def(
        "%parse-source-positioned",
        Arity::exact(1),
        Sig::new(vec![string], map_ty),
        &["s"],
        "Parse s into a CST of maps, each `{:kind :start :end}` (leaves add :text, containers/wrappers add :kids) with half-open character offsets — for structural navigation (std/sexp).",
        parse_source_positioned);
}

pub(super) fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "reflect/read-string", arg(args, 0))?;
    reader::read_one_complete(heap, &s)
}

/// `(reflect/read-first s)` — parse and return the **first** form in `s`, ignoring any
/// trailing forms. The lenient sibling of `reflect/read-string`: for peeking the leading
/// form of a multi-form source (e.g. a file's `(defmodule …)` header) without
/// parsing — or erroring on — the rest.
pub(super) fn read_first(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "reflect/read-first", arg(args, 0))?;
    reader::read_one(heap, &s)
}

/// `(reflect/read-all s)` — parse *every* form in `s` and return them as a list (empty for
/// blank/comment-only input). The all-forms sibling of `reflect/read-string` (which
/// returns only the first), and the read-half of `reflect/eval-string` without the eval —
/// so form-manipulating Brood (an editor evaluating the last sexp before point,
/// say) can isolate individual forms. Raises on a malformed/incomplete form, like
/// `reflect/read-string`; use `parse-source` for lossless, error-tolerant parsing.
pub(super) fn read_all(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "reflect/read-all", arg(args, 0))?;
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
///
/// **Iterative on purpose — an explicit worklist, never Rust recursion.** This walks
/// *reader output*, whose size is bounded only by the file. The reader's `MAX_DEPTH`
/// caps how deeply forms **nest**, but nothing caps how **long** a list is, so a
/// recursive walker that recursed into the cdr had a native-stack depth equal to the
/// list's LENGTH: one generated `.blsp` with a flat ~100k-element list overflowed the
/// stack and SIGABRTed the whole runtime — uncatchable, no `.brood_crash_dump` (a
/// stack overflow is not a panic), taking down every file's `nest check` with it
/// (ADR-119's per-file scan runs through here). A worklist makes the walk O(1) in
/// native stack for both length and nesting.
fn scan_count_syms(heap: &Heap, v: Value, counts: &mut std::collections::HashMap<String, i64>) {
    let mut work = vec![v];
    while let Some(cur) = work.pop() {
        match cur {
            Value::Sym(s) => {
                if let Some(n) = crate::core::value::symbol_name_opt(s) {
                    *counts.entry(n.to_string()).or_insert(0) += 1;
                }
            }
            Value::Pair(id) => {
                let (car, cdr) = heap.pair(id);
                work.push(car);
                work.push(cdr);
            }
            Value::Vector(vid) => {
                work.extend(heap.vector(vid).to_vec());
            }
            Value::Map(mid) => {
                for (k, val) in heap.map_entries(mid) {
                    work.push(k);
                    work.push(val);
                }
            }
            _ => {}
        }
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
