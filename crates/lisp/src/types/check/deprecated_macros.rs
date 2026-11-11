//! Deprecated-macro lint (advisory, `:deprecated`).
//!
//! A `(meta name :deprecated …)` mark on a FUNCTION warns at every call the body walk sees
//! (`walk::stability_msg`). A MACRO's calls never reach that walk: the checker reads
//! macroexpanded code, and by then a call of a `when`-shaped macro is an `if`. So a
//! deprecated macro — the forms ADR-674 retired went through this — would warn nowhere, and
//! the deprecation would end in a removal nobody was told about. This pass walks the
//! **un-expanded** forms, like the guard-purity and discarded-catch passes, and gives a call
//! of a deprecated macro the same diagnostic a call of a deprecated function gets.
//!
//! Quoted data is skipped. A `quasiquote` template is walked: what it builds is code a macro
//! emits. A name the file defines itself is its own, not the deprecated global.
//! `(check-allow :deprecated …)` silences a deliberate use (a shim that calls the name it
//! replaces).

use std::collections::HashSet;

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::types::check::Finding;

use super::walk::{list_items, stability_msg};

/// Entry: walk every top-level (un-expanded) form for a call of a deprecated macro.
pub(super) fn check_deprecated_macros(heap: &Heap, forms: &[Value], out: &mut Vec<Finding>) {
    let mut defined = HashSet::new();
    for &form in forms {
        collect_defined(heap, form, &mut defined);
    }
    for &form in forms {
        walk(heap, form, &defined, out);
    }
}

fn walk(heap: &Heap, form: Value, defined: &HashSet<String>, out: &mut Vec<Finding>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            if let Value::Vector(id) = form {
                for &it in heap.vector(id).iter() {
                    walk(heap, it, defined, out);
                }
            }
            return;
        };
        if let Some(&Value::Sym(head)) = items.first() {
            if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::COMMENT) {
                return;
            }
            if (value::symbol_is(head, "check-allow") || value::symbol_is(head, "%lint-allow"))
                && matches!(items.get(1), Some(&Value::Keyword(category))
                    if value::symbol_is(category, "deprecated"))
            {
                return;
            }
            let name = value::symbol_name(head);
            let bare = name
                .strip_prefix('/')
                .filter(|n| !n.is_empty())
                .unwrap_or(&name);
            if !defined.contains(bare) {
                let sym = value::intern(bare);
                if matches!(
                    crate::types::check::deps::obs_global(heap, sym),
                    Some(Value::Macro(_))
                ) && let Some((lint, msg)) = stability_msg(heap, sym)
                {
                    out.push(Finding::new(lint, heap.form_pos_only(form), msg));
                }
            }
        }
        for &child in &items {
            walk(heap, child, defined, out);
        }
    })
}

/// The names the file defines itself, at any depth: a module's own definition of a
/// deprecated name is not the deprecated global.
fn collect_defined(heap: &Heap, form: Value, out: &mut HashSet<String>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let (Some(&Value::Sym(head)), Some(&Value::Sym(name))) = (items.first(), items.get(1))
            && matches!(
                value::symbol_name_ref(head),
                "def"
                    | "def-"
                    | "defn"
                    | "defn-"
                    | "defmacro"
                    | "defmacro-"
                    | "defdyn"
                    | "defmulti"
            )
        {
            out.insert(value::symbol_name(name));
        }
        for &it in &items {
            collect_defined(heap, it, out);
        }
    })
}
