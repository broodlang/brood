//! Discarded-catch lint (advisory, `:discarded-catch`).
//!
//! A `(try … (catch e nil))` whose handler is a constant cannot have read the
//! error it caught, so whatever was raised — an **unbound symbol** after a rename,
//! a real fault — is swallowed unseen. A downstream project ran for hours with ten
//! unbound references because each sat in exactly this shape around a GUI call:
//! the editor came up unfonted and untitled, and nothing said why.
//!
//! Walks the **un-expanded** forms, like the `match`-exhaustiveness and guard-purity
//! passes: a `catch` clause survives only pre-expansion (`try` is a macro over `%try`,
//! and the handler becomes an anonymous `fn`), and — more to the point — only the
//! surface form separates a catch the *author* wrote from one a macro built.
//! `assert-error` expands to `(try (do … false) (catch e true))`, where the
//! constant IS the assertion; linting after expansion flagged every one of its
//! ~350 uses in `tests/` while saying nothing about the macro the author never sees.
//!
//! The rule is syntactic on purpose. It fires when the handler body is **empty or a
//! single constant** — `nil`/`true`/`false`, a number, a string, a keyword, a
//! `(quote …)`, or an empty `(do)` — regardless of the binding's spelling: `(catch _
//! nil)` is the pattern, not an opt-out. A body with any call in it, even one that
//! ignores the binding (`(catch _ (fallback))`), is a deliberate fallback that works
//! and stays silent. Where the constant is genuinely the answer — probing whether a
//! feature exists, an "did it throw?" test — the author says so with
//! `(check-allow :discarded-catch …)`, which this pass reads in its surface spelling
//! (and as its `%lint-allow` expansion, should one reach it).

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::error::Pos;

use super::walk::list_items;

/// The prefix every discarded-catch diagnostic starts with (tests match on it).
pub(super) const DISCARDED_CATCH_PREFIX: &str = "catch discards the error unread";

/// Entry: walk every top-level (un-expanded) form for a `try` whose `catch` discards.
pub(super) fn check_discarded_catches(
    heap: &Heap,
    forms: &[Value],
    out: &mut Vec<(Option<Pos>, String)>,
) {
    for &form in forms {
        walk(heap, form, out);
    }
}

fn walk(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    // Deep-form stack safety — the same stacker remedy as the other raw-form passes.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || walk_inner(heap, form, out))
}

fn walk_inner(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    if let Some(&Value::Sym(head)) = items.first() {
        // Quoted subtrees are data; a `quasiquote` is a macro template whose catches
        // belong to the expansion site, not to this file; a `comment` never runs.
        if value::symbol_is(head, kw::QUOTE)
            || value::symbol_is(head, kw::QUASIQUOTE)
            || value::symbol_is(head, kw::COMMENT)
        {
            return;
        }
        // `(check-allow :discarded-catch …)` / its `%lint-allow` expansion: the author
        // has said the constant is the answer. Skip the whole subtree. A different
        // category falls through and is walked normally.
        if (value::symbol_is(head, "check-allow") || value::symbol_is(head, "%lint-allow"))
            && matches!(items.get(1), Some(&Value::Keyword(category))
                if value::symbol_is(category, "discarded-catch"))
        {
            return;
        }
        if value::symbol_is(head, kw::TRY) {
            lint_try(heap, form, &items, out);
        }
    }
    for &child in &items {
        walk(heap, child, out);
    }
}

/// `(try body… (catch BINDING handler…))` — warn when the handler is empty or one
/// constant. A `try` with no `catch` clause, or a malformed one, is not this lint's
/// business (the macro reports the malformed shapes itself).
fn lint_try(heap: &Heap, form: Value, items: &[Value], out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&clause) = items.last() else { return };
    let Some(clause_items) = list_items(heap, clause) else {
        return;
    };
    let [Value::Sym(catch), Value::Sym(binding), handler @ ..] = clause_items.as_slice() else {
        return;
    };
    if !value::symbol_is(*catch, "catch") {
        return;
    }
    let discards = match handler {
        [] => true,
        [single] => is_constant_form(heap, *single),
        _ => false,
    };
    if !discards {
        return;
    }
    let binding_name = value::symbol_name(*binding);
    out.push((
        heap.form_pos_only(clause)
            .or_else(|| heap.form_pos_only(form)),
        format!(
            "{DISCARDED_CATCH_PREFIX}: (catch {binding_name} …) with a constant body hides an \
             unbound symbol or a real fault; inspect `{binding_name}` (error-message) or \
             narrow what you catch — or, where the error IS the answer, say so with \
             (check-allow :discarded-catch …)"
        ),
    ));
}

/// Is `v` a handler result that says NOTHING — `nil`, `false`, or an empty `(do)`?
/// Those are indistinguishable from "there was no value", which is how a swallowed
/// unbound symbol hides. A keyword, `true`, a string or a number is a *sentinel* the
/// author chose (`(catch e :raised)`, `(catch _ true)` — "it threw" as a value) and
/// carries intent, so it is not flagged: the did-it-throw assertion is idiomatic, and
/// flagging it made the lint fire 128 times over tests for zero findings.
fn is_constant_form(heap: &Heap, v: Value) -> bool {
    match v {
        Value::Nil | Value::Bool(false) => true,
        Value::Pair(_) => match list_items(heap, v).as_deref() {
            Some([Value::Sym(h)]) => value::symbol_is(*h, kw::DO),
            _ => false,
        },
        _ => false,
    }
}
