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

use super::walk::list_items;

/// Primitives that perform an effect — message passing / process control, `Table` mutation
/// (Brood's one identity-mutable structure, ADR-107), I/O, or global rebinding / effectful
/// metaprogramming. The complement is the guard-safe subset (comparisons, type/shape
/// predicates, total arithmetic, pure data reads) that every guard in `std/` already uses.
const EFFECTFUL_IN_GUARD: &[&str] = &[
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
    for &form in forms {
        walk(heap, form, out);
    }
}

/// Recurse un-expanded code, skipping quoted data. At each list, apply the two guard shapes:
/// a clause `(pattern :when GUARD body…)` (`:when` at index 1 — `match`/`receive`/`case`
/// clauses and multi-clause `fn`/`defn` clauses), and a single-clause `fn`/`defn` whose
/// `:when` follows the parameter list.
fn walk(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        // Quoted subtrees are data (patterns / literals), not code — never guards.
        if matches!(items.first(), Some(&Value::Sym(h))
            if value::symbol_is(h, "quote") || value::symbol_is(h, "quasiquote"))
        {
            return;
        }
        // Shape 1: a clause `(pat :when GUARD …)` — `:when` in the second position.
        if is_when_kw(items.get(1)) {
            if let Some(&guard) = items.get(2) {
                lint_guard(heap, guard, out);
            }
        }
        // Shape 2: a single-clause `(fn (params) :when GUARD …)` /
        // `(defn name (params) :when GUARD …)`.
        if let Some(&Value::Sym(h)) = items.first() {
            let guard_index = match value::symbol_name(h).as_str() {
                "fn" | "lambda" if is_when_kw(items.get(2)) => Some(3),
                "defn" | "defmacro" if is_when_kw(items.get(3)) => Some(4),
                _ => None,
            };
            if let Some(i) = guard_index {
                if let Some(&guard) = items.get(i) {
                    lint_guard(heap, guard, out);
                }
            }
        }
        for &it in &items {
            walk(heap, it, out);
        }
    })
}

fn is_when_kw(v: Option<&Value>) -> bool {
    matches!(v, Some(&Value::Keyword(k)) if value::symbol_is(k, "when"))
}

/// Warn if `guard` contains an effectful primitive.
fn lint_guard(heap: &Heap, guard: Value, out: &mut Vec<(Option<Pos>, String)>) {
    if let Some(name) = effectful_head(heap, guard) {
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

/// The first effectful primitive head anywhere in `form`, or `None`. Skips quoted data.
fn effectful_head(heap: &Heap, form: Value) -> Option<String> {
    let items = list_items(heap, form)?;
    if let Some(&Value::Sym(h)) = items.first() {
        let name = value::symbol_name(h);
        if name == "quote" || name == "quasiquote" {
            return None;
        }
        if EFFECTFUL_IN_GUARD.contains(&name.as_str()) {
            return Some(name);
        }
    }
    for &it in &items {
        if let Some(n) = effectful_head(heap, it) {
            return Some(n);
        }
    }
    None
}
