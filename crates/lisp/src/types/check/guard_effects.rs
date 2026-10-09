//! Guard-purity lint (advisory) — a `:when` guard must be a *decision*, not an *action*.
//!
//! A guard runs on clauses the match **rejects** — an earlier clause whose pattern matches
//! but whose guard is false has already evaluated the guard before the next clause is tried —
//! and in a `receive` it re-runs against **every scanned message on each mailbox re-scan**
//! (the scan restarts at the front on every suspend/resume). So an effect in a guard fires on
//! paths the match never selects, and a `receive` guard's effect fires repeatedly against
//! messages it never consumes. LFE/Erlang restrict guards to a pure, total sublanguage for
//! exactly this reason.
//!
//! This pass flags message-passing / process-control, `Table`-mutation, I/O, and
//! global-rebinding primitives in a guard. It is **advisory and purely syntactic** (no type
//! context needed) and walks the **un-expanded** forms — a `:when` guard survives only
//! pre-expansion (like `match`), since expansion lowers it into an ordinary `if`-test.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::error::Pos;
use std::collections::HashSet;

use super::walk::list_items;

/// Primitives that perform an effect — message passing / process control, `Table` mutation
/// (Brood's one identity-mutable structure, ADR-107), I/O, or global rebinding / effectful
/// metaprogramming. The complement is the guard-safe subset (comparisons, type/shape
/// predicates, total arithmetic, pure data reads) that every guard in `std/` already uses.
pub(super) const EFFECTFUL_IN_GUARD: &[&str] = &[
    // message passing / process control
    "send",
    "spawn",
    "spawn-link",
    "exit",
    "link",
    "unlink",
    "monitor",
    "demonitor",
    // Table mutation — the one identity-mutable structure
    kw::TABLE_PUT,
    kw::TABLE_INCR,
    kw::TABLE_ADD,
    kw::TABLE_SUB,
    kw::TABLE_DELETE,
    kw::TABLE_DROP,
    // I/O. These carried the pre-namespacing spellings (`println`, `print`, `os-cmd`,
    // `run-process`, `halt`) long after the names moved, so the checker had silently
    // stopped recognising them — a stale entry here is not an error anywhere, it just
    // stops flagging. Kept as the qualified names the modules actually export.
    "io/puts",
    "io/write",
    "file/spit",
    "file/spit-append",
    "file/slurp",
    "read-line",
    "os/cmd",
    "os/run-process",
    "os/spawn",
    "system/halt",
    // global rebinding / effectful metaprogramming
    "def",
    "defn",
    "defmacro",
    "reflect/eval",
    "reflect/load",
    "require-one",
];

/// Entry: walk every top-level form for effectful `:when` guards.
pub(super) fn check_guards(heap: &Heap, forms: &[Value], out: &mut Vec<(Option<Pos>, String)>) {
    // A name the file defines itself shadows the global of that spelling: a module's own
    // `(defn exit (s) …)` is what a bare `exit` in its guards calls.
    let mut defined = HashSet::new();
    for &form in forms {
        collect_defined_names(heap, form, &mut defined);
    }
    for &form in forms {
        walk(heap, form, &defined, out);
    }
}

/// The heads that bind their binding list's targets for the rest of the form.
const LET_LIKE: &[&str] = &["let", "letrec", "let*", "if-let", "when-let", "for", "loop"];

/// The heads whose clauses are `(pattern body…)`, the pattern binding for its clause.
const CLAUSE_FORMS: &[&str] = &["match", "match*", "receive", "case"];

/// Recurse un-expanded code, skipping quoted data. At each list, apply the two guard shapes:
/// a clause `(pattern :when GUARD body…)` (`:when` at index 1 — `match`/`receive`/`case`
/// clauses and multi-clause `fn`/`defn` clauses), and a single-clause `fn`/`defn` whose
/// `:when` follows the parameter list.
///
/// `scope` is every name bound around `form` — the file's own definitions plus each
/// enclosing binder's names — so a guard calling a LOCAL `send` (a `let`-bound predicate,
/// a parameter, a module function of that name) is not read as the primitive. Binders are
/// read generously (every symbol in a pattern counts); a name wrongly counted as bound only
/// costs a missed warning, never a false one.
fn walk(heap: &Heap, form: Value, scope: &HashSet<String>, out: &mut Vec<(Option<Pos>, String)>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            if let Value::Vector(id) = form {
                for &it in heap.vector(id).iter() {
                    walk(heap, it, scope, out);
                }
            }
            return;
        };
        // Quoted subtrees are data (patterns / literals), not code — never guards.
        if matches!(items.first(), Some(&Value::Sym(h))
            if value::symbol_is(h, "quote") || value::symbol_is(h, "quasiquote"))
        {
            return;
        }
        let head_name = match items.first() {
            Some(&Value::Sym(h)) => Some(value::symbol_name(h)),
            _ => None,
        };
        let head_name = head_name.as_deref();
        // The names this form binds for its children.
        let mut inner = scope.clone();
        match head_name {
            Some(name) if LET_LIKE.contains(&name) => {
                if let Some(&bindings) = items.get(1) {
                    for target in binding_targets(heap, bindings) {
                        collect_symbols(heap, target, &mut inner);
                    }
                }
            }
            Some("fn" | "lambda") => {
                if let Some(&params) = items.get(1) {
                    collect_symbols(heap, params, &mut inner);
                }
            }
            Some("defn" | "defn-" | "defmacro") => {
                if let Some(&params) = items.get(2) {
                    collect_symbols(heap, params, &mut inner);
                }
            }
            _ => {}
        }
        // Shape 1: a clause `(pat :when GUARD …)` — `:when` in the second position. The
        // pattern's binders are in scope for the guard (and the body).
        if is_when_kw(items.get(1)) {
            collect_symbols(heap, items[0], &mut inner);
            if let Some(&guard) = items.get(2) {
                lint_guard(heap, guard, &inner, out);
            }
        }
        // Shape 2: a single-clause `(fn (params) :when GUARD …)` /
        // `(defn name (params) :when GUARD …)`.
        let guard_index = match head_name {
            Some("fn" | "lambda") if is_when_kw(items.get(2)) => Some(3),
            Some("defn" | "defmacro") if is_when_kw(items.get(3)) => Some(4),
            _ => None,
        };
        if let Some(i) = guard_index {
            if let Some(&guard) = items.get(i) {
                lint_guard(heap, guard, &inner, out);
            }
        }
        let clause_form = head_name.is_some_and(|name| CLAUSE_FORMS.contains(&name));
        for (index, &it) in items.iter().enumerate() {
            // A clause `(pattern body…)` of a `match`: its pattern binds for its body.
            if clause_form && index >= 2 {
                if let Some(clause) = list_items(heap, it) {
                    if let Some(&pattern) = clause.first() {
                        let mut clause_scope = inner.clone();
                        collect_symbols(heap, pattern, &mut clause_scope);
                        walk(heap, it, &clause_scope, out);
                        continue;
                    }
                }
            }
            walk(heap, it, &inner, out);
        }
    })
}

/// The targets of a binding list `(t1 v1 t2 v2 …)` / `[t1 v1 …]` — its even elements.
fn binding_targets(heap: &Heap, bindings: Value) -> Vec<Value> {
    let elements = match bindings {
        Value::Vector(id) => heap.vector(id).to_vec(),
        _ => list_items(heap, bindings).unwrap_or_default(),
    };
    elements.into_iter().step_by(2).collect()
}

/// Every symbol anywhere in `form` (a parameter list or a pattern), by name.
fn collect_symbols(heap: &Heap, form: Value, out: &mut HashSet<String>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form {
        Value::Sym(symbol) => {
            out.insert(value::symbol_name(symbol));
        }
        Value::Pair(_) => {
            for part in list_items(heap, form).unwrap_or_default() {
                collect_symbols(heap, part, out);
            }
        }
        Value::Vector(id) => {
            for &part in heap.vector(id).iter() {
                collect_symbols(heap, part, out);
            }
        }
        Value::Map(id) => {
            for (key, part) in heap.map_entries(id) {
                collect_symbols(heap, key, out);
                collect_symbols(heap, part, out);
            }
        }
        _ => {}
    })
}

/// Every name the file defines with a `def`-family form, at any depth.
fn collect_defined_names(heap: &Heap, form: Value, out: &mut HashSet<String>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let (Some(&Value::Sym(head)), Some(&Value::Sym(name))) = (items.first(), items.get(1)) {
            let head = value::symbol_name(head);
            if matches!(
                head.as_str(),
                "def" | "def-" | "defn" | "defn-" | "defmacro" | "defdyn" | "defmulti"
            ) {
                out.insert(value::symbol_name(name));
            }
        }
        for &it in &items {
            collect_defined_names(heap, it, out);
        }
    })
}

fn is_when_kw(v: Option<&Value>) -> bool {
    matches!(v, Some(&Value::Keyword(k)) if value::symbol_is(k, "when"))
}

/// Warn if `guard` contains an effectful primitive.
fn lint_guard(
    heap: &Heap,
    guard: Value,
    scope: &HashSet<String>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if let Some(name) = effectful_head(heap, guard, scope) {
        out.push((
            heap.form_pos_only(guard),
            format!(
                "effectful call `{name}` in a `:when` guard — a guard is a decision, not an \
                 action. It runs even on clauses the match rejects, and in a `receive` it \
                 re-runs against every scanned message on each mailbox re-scan, so the effect \
                 fires on paths never selected. Move it into the clause body."
            ),
        ));
    }
}

/// The first effectful primitive head anywhere in `form`, or `None`. Skips quoted data, and
/// a head `scope` binds — that name is not the primitive.
fn effectful_head(heap: &Heap, form: Value, scope: &HashSet<String>) -> Option<String> {
    // Deep-form stack safety: a generated guard is as deep as its generator made it.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        effectful_head_inner(heap, form, scope)
    })
}

fn effectful_head_inner(heap: &Heap, form: Value, scope: &HashSet<String>) -> Option<String> {
    let items = list_items(heap, form)?;
    if let Some(&Value::Sym(h)) = items.first() {
        let name = value::symbol_name(h);
        if name == "quote" || name == "quasiquote" {
            return None;
        }
        if EFFECTFUL_IN_GUARD.contains(&name.as_str()) && !scope.contains(&name) {
            return Some(name);
        }
    }
    for &it in &items {
        if let Some(n) = effectful_head(heap, it, scope) {
            return Some(n);
        }
    }
    None
}
