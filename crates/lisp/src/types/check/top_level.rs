//! Readers of a file's top-level definition forms: the definitions with the `do` that
//! `defn-`, typed parameters and `check-allow` wrap them in opened up, a `(def NAME RHS)`
//! split into its parts, the names a form guards with `(bound? 'name)`, and how many times
//! each name is `def`'d anywhere.

use super::*;

/// The top-level definition forms of a file — each form as written, with the `(do …)`
/// that `defn-`/`def-` expand to opened up one level.
///
/// `defn-`/`def-` (ADR-146) expand to `(do (def name …) (%mark-private 'name))`, so a
/// pass reading `expanded` directly finds no definition at all for a **module-private**
/// function — and most definitions in a real module are private (40 of
/// `std/json.blsp`'s 42). A pass keyed on [`def_name_and_value`] therefore has to look
/// inside, or it silently declines to infer anything about them and their call sites go
/// unchecked.
///
/// **Only** that shape, identified by the `%mark-private` call it carries. Other macros
/// emit a top-level `do` too, and opening those was tried and reverted — twice over:
///
/// - the linear-map rewrite wraps a fold's result in `(do (def linmap-out__N …) …)`, and
///   typing that temporary made the checker flag a branch of the rewrite's own wrapper
///   that cannot run with that value — a warning naming a symbol the author never wrote
///   (caught live by `tests/linmap_soundness_test.blsp`); and
/// - `defability`/`defimpl` define their ops inside one, and an *inferred* signature for
///   an op displaces the `:-> T` return the ability declares, so the return stopped
///   flowing to call sites (`ability_op_return_type_flows_to_call_site`).
///
/// Both consumers additionally skip [`value::is_gensym`] names on their own account: a
/// gensym is invisible to the author, so a diagnostic naming one is never actionable.
pub(super) fn top_level_defs(heap: &Heap, expanded: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(expanded.len());
    for &form in expanded {
        // A `(check-allow :category …)` around a definition is the author suppressing a
        // lint, not hiding the definition: its `def` is inferred like any other, so its
        // callers read a return rather than `any`. `:generated` stays opaque — the copy the linear-map rewrite keeps
        // is not the author's code (KI-152).
        collect_top_level_def(heap, form, &mut out);
    }
    out
}

fn collect_top_level_def(heap: &Heap, form: Value, out: &mut Vec<Value>) {
    if let Some(wrapped) = lint_allowed_body(heap, form) {
        // `check-allow` wraps its forms in one `do`: each of them is a top-level form.
        for &inner in &wrapped {
            collect_top_level_def(heap, inner, out);
        }
        return;
    }
    // A `defn-` (`(do (def …) (%mark-private …))`) or a definition with typed parameters
    // (`(do (def …) (%register-sig …))`, ADR-420) is the definition beside its marks, not a
    // block that hides it: unwrap it, recursively — a typed `defn-` nests one in the other.
    match walk::do_body(heap, form)
        .filter(|body| marks_private(heap, body) || registers_sig(heap, body))
    {
        Some(body) => {
            for inner in body {
                collect_top_level_def(heap, inner, out);
            }
        }
        None => out.push(form),
    }
}

/// Does this `do` body carry a `(%register-sig …)` call beside a `def` — the expansion of a
/// definition with typed parameters (ADR-420)?
fn registers_sig(heap: &Heap, body: &[Value]) -> bool {
    let head_is = |form: Value, name: &str| {
        list_items(heap, form)
            .and_then(|items| items.first().copied())
            .is_some_and(|head| matches!(head, Value::Sym(s) if value::symbol_is(s, name)))
    };
    body.iter().any(|&f| head_is(f, "%register-sig")) && body.iter().any(|&f| head_is(f, "def"))
}

/// The forms a `(%lint-allow :category (do FORM…))` wraps, for every category but
/// `:generated`; `None` for anything else.
fn lint_allowed_body(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    let items = list_items(heap, form)?;
    let [Value::Sym(head), Value::Keyword(category), wrapped] = items.as_slice() else {
        return None;
    };
    if !value::symbol_is(*head, "%lint-allow") || value::symbol_is(*category, "generated") {
        return None;
    }
    Some(walk::do_body(heap, *wrapped).unwrap_or_else(|| vec![*wrapped]))
}

/// Does this `do` body carry a `(%mark-private …)` call — the fingerprint of a
/// `defn-`/`def-` expansion?
fn marks_private(heap: &Heap, body: &[Value]) -> bool {
    body.iter().any(|&form| {
        list_items(heap, form)
            .and_then(|items| items.first().copied())
            .is_some_and(
                |head| matches!(head, Value::Sym(s) if value::symbol_is(s, "%mark-private")),
            )
    })
}

/// A `(def NAME RHS)` form → `(NAME, RHS)`. `None` for anything else — a non-`def` head,
/// a `(defmacro …)` (a special form, never expanded to `def`), or a malformed def.
pub(super) fn def_name_and_value(heap: &Heap, form: Value) -> Option<(Symbol, Value)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, kw::DEF) {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    Some((name, items[2]))
}

/// Every name `form` guards with an explicit `(bound? 'name)` test, recursively.
///
/// A `(bound? 'x)` test says, in the source itself, "this name may not exist in this
/// image" — the prelude's ability dispatch reads `project/*name*` / `project/*ns-package*` that
/// way, since those ambients are `defdyn`'d by `std/tool/project.blsp` and simply absent
/// under a bare `brood script.blsp`. Flagging the guarded reference as unbound reports a
/// *correct* program, which is exactly what the advisory contract forbids
/// (ADR-123..126) — so the guarded name is exempt for the top-level form that guards it.
///
/// Scoped to one top-level form (usually one `defn`) rather than the whole file so a
/// `bound?` probe in one function can't silence a genuine typo in another.
pub(super) fn collect_bound_guards(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(head)) = items.first()
            && value::symbol_is(head, "bound?")
        {
            // `(bound? 'name)` reads as `(bound? (quote name))` in the expanded tree.
            if let Some(quoted) = items.get(1).copied().and_then(|q| list_items(heap, q))
                && matches!(quoted.first(), Some(&Value::Sym(q)) if value::symbol_is(q, kw::QUOTE))
                && let Some(&Value::Sym(name)) = quoted.get(1)
            {
                out.insert(name);
            }
        }
        for &item in items.iter() {
            collect_bound_guards(heap, item, out);
        }
    })
}

/// Count every `(def NAME …)` for each name across `form`, **including nested** defs (a
/// reassignment inside a function body, a `do`, a `when`, …) — not just top-level. A global
/// def'd more than once anywhere is *reassigned*, so its type isn't its first value; the
/// value-type inference (Gap A) and the same-file return inference (same-file inference) both use this
/// to leave such a global `dynamic()` rather than pinning it to a stale initial value (the
/// lazy-init `(when (nil? *g*) (def *g* (table)))` pattern — else a function returning `*g*`
/// infers `nil` and false-flags its callers). Skips quoted subtrees.
pub(super) fn count_defs(heap: &Heap, form: Value, counts: &mut HashMap<Symbol, usize>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
            return;
        }
        if value::symbol_is(head, kw::DEF)
            && let Some(&Value::Sym(name)) = items.get(1)
        {
            *counts.entry(name).or_insert(0) += 1;
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            count_defs(heap, item, counts);
        }
    })
}
