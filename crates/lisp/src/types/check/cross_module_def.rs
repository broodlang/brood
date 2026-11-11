//! Cross-module `def` lint (advisory, `:cross-module-def`).
//!
//! A module that, while it LOADS, re-binds a global another module owns —
//! `(def theme/*chrome-faces* (conj theme/*chrome-faces* row))` — is a read-modify-write
//! on shared state with no lock. Under the parallel cold load (ADR-426) two modules doing
//! it at once each read the old value and the later `def` drops the earlier one's write.
//! bedit's theme registered every feature's faces this way, and an image built in the pool
//! lost whole modules' worth of them (a theme switch left git's diff colours on the old
//! palette, in some builds and not others). The write is also invisible to the startup
//! image, which carries what loading creates, not what it mutates (ADR-218). `std/registry`
//! (`registry/put!`) is the shape that has none of this: one locked write, carried by the
//! image.
//!
//! Walks the **expanded** forms, unlike the other shape lints: the offending `def` is
//! usually written inside a macro's template in the module that OWNS the global (where it
//! is no cross-module write at all) and becomes one only where the macro is used. Only
//! load-time code is read — a top-level form and what it runs directly — never a `fn` body,
//! which runs when it is called, nor a quoted form, which is data.
//!
//! Only a global that is ALREADY bound is a re-binding: a fresh name with a slash in it (the
//! linmap rewrite's generated `fn/linmap-loop__N`) defines something new, not another
//! module's state.
//!
//! A target is the file's own when its namespace is one of the file's `defmodule` names (or
//! ends with one, for the project-qualified spelling `bedit/theme/…`). A file with no
//! `defmodule` defines root globals and owns no namespace, so it is not linted. Where the
//! write is genuinely safe (one writer, loaded before anything reads it), the author says so
//! with `(check-allow :cross-module-def …)`.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::types::check::{Finding, Lint};

use super::deps;
use super::walk::list_items;

/// The prefix every cross-module-def diagnostic starts with (tests match on it).
pub(super) const CROSS_MODULE_DEF_PREFIX: &str = "a load-time def of another module's global";

/// Entry: walk every top-level expanded form for a `def` of a global another module owns.
/// `own` holds the names of the modules the file declares; empty means no `defmodule`.
pub(super) fn check_cross_module_defs(
    heap: &Heap,
    expanded: &[Value],
    own: &[String],
    out: &mut Vec<Finding>,
) {
    if own.is_empty() {
        return;
    }
    for &form in expanded {
        walk(heap, form, form, own, out);
    }
}

fn walk(heap: &Heap, form: Value, top: Value, own: &[String], out: &mut Vec<Finding>) {
    // Deep-form stack safety — the same stacker remedy as the other raw-form passes.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        walk_inner(heap, form, top, own, out)
    })
}

fn walk_inner(heap: &Heap, form: Value, top: Value, own: &[String], out: &mut Vec<Finding>) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    if let Some(&Value::Sym(head)) = items.first() {
        // Data, and code that runs later: a quoted form, a function body, a macro.
        if value::symbol_is(head, kw::QUOTE)
            || value::symbol_is(head, kw::QUASIQUOTE)
            || value::symbol_is(head, kw::FN)
            || value::symbol_is(head, kw::DEFMACRO)
        {
            return;
        }
        if (value::symbol_is(head, "check-allow") || value::symbol_is(head, "%lint-allow"))
            && matches!(items.get(1), Some(&Value::Keyword(category))
                if value::symbol_is(category, "cross-module-def"))
        {
            return;
        }
        if value::symbol_is(head, kw::DEF)
            && let Some(&Value::Sym(target)) = items.get(1)
            && let name = value::symbol_name(target)
            && let Some(owner) = foreign_owner(&name, own)
            && already_bound(heap, target)
        {
            out.push(Finding::new(
                Lint::CrossModuleDef,
                heap.form_pos_only(form).or_else(|| heap.form_pos_only(top)),
                format!(
                    "{CROSS_MODULE_DEF_PREFIX}: `{name}` belongs to `{owner}`, and this \
                     module re-binds it while it loads — two modules doing that under the \
                     parallel load each read the old value and one write is lost, and the \
                     startup image does not carry it. Register through std `registry` \
                     (`registry/put!`), or, where this module is the only writer, say so \
                     with (check-allow :cross-module-def …)"
                ),
            ));
        }
    }
    for &child in &items {
        walk(heap, child, top, own, out);
    }
}

/// The namespace that owns `name` when it is qualified and that namespace is not one of
/// `own` (nor a project-qualified spelling of one, `bedit/theme` for `theme`); else `None`.
/// The division operator `/` and a name ending in `/` are not qualified names.
fn foreign_owner<'a>(name: &'a str, own: &[String]) -> Option<&'a str> {
    let slash = name.rfind('/')?;
    if slash == 0 || slash + 1 == name.len() {
        return None;
    }
    let ns = &name[..slash];
    let is_own = own
        .iter()
        .any(|module| ns == module || ns.ends_with(&format!("/{module}")));
    if is_own { None } else { Some(ns) }
}

/// Whether `target` names a global that is already defined — the owner has loaded and
/// bound it, so this `def` RE-binds it, which is the read-modify-write that loses updates.
/// A fresh name in a foreign-looking namespace is not one: the linmap rewrite names its
/// generated helpers `fn/linmap-loop__N`, and those are new definitions, not another
/// module's state. A project file spells another project module's global unrooted
/// (`theme/rows` for `bedit/theme/rows`, ADR-070), so the rooted spelling is tried too.
fn already_bound(heap: &Heap, target: value::Symbol) -> bool {
    deps::obs_global(heap, target).is_some()
        || heap
            .root_qualified_ref(target)
            .is_some_and(|rooted| deps::obs_global(heap, rooted).is_some())
}
