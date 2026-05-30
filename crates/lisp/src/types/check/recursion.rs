//! Non-tail self-recursion lint (advisory).
//!
//! Brood has no `while`/`for`; loops are recursion, and **tail** calls run in
//! O(1) stack (the eval `'tail:` loop). A function that calls *itself* in a
//! **non-tail** position therefore grows the native/coroutine stack per
//! iteration and overflows on deep input — a silent footgun that only bites at
//! depth. This pass flags the obvious cases so they surface at write time
//! (via `nest check` and, since it rides the same diagnostic channel, live in
//! the LSP).
//!
//! Runs over the **macroexpanded** forms (like the rest of the checker), so
//! `match`/`case`/threading macros are already lowered to the core special
//! forms and a `defn` is `(def name (fn …))`. That leaves a small, fixed set of
//! tail-propagating special forms to model — `if` / `when` / `unless` / `cond`
//! / `do` / `let` / `let*` / `letrec` / `and` / `or` — mirroring the evaluator's
//! `'tail:` handling. **Conservative by design:** it descends only into forms
//! whose tail structure it knows, stops at nested `fn`/`lambda` (a self-call
//! inside an inner closure is a different frame) and at `quote`/`quasiquote`
//! (data), and only flags a self-call it is *certain* sits in non-tail
//! position. It would rather miss a case than warn on correct code.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Value};
use crate::error::Pos;

use super::walk::list_items;

/// Entry: find every `(def NAME (fn …))` anywhere in `form` and check each for
/// self-calls to `NAME` in non-tail position.
pub(super) fn check_recursion(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    if let (Some(&Value::Sym(head)), true) = (items.first(), items.len() >= 3) {
        if value::symbol_is(head, kw::DEF) {
            if let Value::Sym(name) = items[1] {
                if is_fn_form(heap, items[2]) {
                    analyze_fn(heap, name, items[2], out);
                }
            }
        }
    }
    // Recurse so nested definitions (a `defn` inside `do` / `test` / `describe`,
    // a closure-defining `def` in a body) are covered too.
    for &child in &items {
        check_recursion(heap, child, out);
    }
}

/// Is `v` a `(fn …)` / `(lambda …)` form?
fn is_fn_form(heap: &Heap, v: Value) -> bool {
    matches!(list_items(heap, v).as_deref(),
        Some([Value::Sym(h), ..]) if value::symbol_is(*h, kw::FN) || value::symbol_is(*h, kw::LAMBDA))
}

/// Analyze a `(fn …)` value: one body for a single-arity fn, or each arm's body
/// for a multi-arity fn. The fn body's last form is in tail position.
fn analyze_fn(heap: &Heap, name: Symbol, fnval: Value, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(items) = list_items(heap, fnval) else {
        return;
    };
    // items = [fn, params-or-arm, ...]. Multi-arity iff the element after `fn`
    // is a clause `((params) body…)` — i.e. its own first element is a list.
    // (Pattern clauses are already lowered to `match*`, so post-expansion arms
    // are arity-only: their params are a plain symbol list.)
    let multi = matches!(items.get(1), Some(&arm) if first_is_list(heap, arm));
    if multi {
        for &arm in &items[1..] {
            // arm = (params body…): body is everything after the param list.
            if let Some(arm_items) = list_items(heap, arm) {
                analyze_body(heap, name, &arm_items[1..], out);
            }
        }
    } else {
        // (fn params body…): body is everything after the param list.
        analyze_body(heap, name, &items[2..], out);
    }
}

/// True if `v` is a list whose first element is itself a list (the multi-arity
/// clause shape). A single-arity param list's first element is a symbol or a
/// vector pattern, never a bare list.
fn first_is_list(heap: &Heap, v: Value) -> bool {
    matches!(list_items(heap, v).as_deref(), Some([first, ..]) if matches!(first, Value::Pair(_)))
}

/// A body (implicit `do`): the last form is in tail position, the rest are not.
fn analyze_body(heap: &Heap, name: Symbol, body: &[Value], out: &mut Vec<(Option<Pos>, String)>) {
    if body.is_empty() {
        return;
    }
    let last = body.len() - 1;
    for (i, &form) in body.iter().enumerate() {
        walk(heap, form, i == last, name, out);
    }
}

/// Walk `form`, knowing whether it is in tail position, flagging any non-tail
/// call to `name`.
fn walk(heap: &Heap, form: Value, tail: bool, name: Symbol, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(items) = list_items(heap, form) else {
        return; // atom — not a call
    };
    let Some(&first) = items.first() else {
        return; // ()
    };

    if let Value::Sym(head) = first {
        // Forms we don't enter: data, and inner closures (a different frame).
        if value::symbol_is(head, kw::QUOTE)
            || value::symbol_is(head, kw::QUASIQUOTE)
            || value::symbol_is(head, kw::FN)
            || value::symbol_is(head, kw::LAMBDA)
        {
            return;
        }
        // Tail-propagating special forms — mirror the evaluator's `'tail:` rules.
        if value::symbol_is(head, kw::IF) {
            // (if test then else?): test is non-tail; then/else inherit `tail`.
            if let Some(&t) = items.get(1) {
                walk(heap, t, false, name, out);
            }
            for &branch in items.iter().skip(2) {
                walk(heap, branch, tail, name, out);
            }
            return;
        }
        if value::symbol_is(head, kw::WHEN) || value::symbol_is(head, kw::UNLESS) {
            // (when test body…): test non-tail; body is an implicit `do`.
            if let Some(&t) = items.get(1) {
                walk(heap, t, false, name, out);
            }
            analyze_body(heap, name, &items[2..], out);
            return;
        }
        if value::symbol_is(head, kw::DO)
            || value::symbol_is(head, kw::AND)
            || value::symbol_is(head, kw::OR)
        {
            // do: last form is tail. and/or: the last operand is the result
            // (tail); earlier operands are tested (non-tail). Same shape.
            analyze_body(heap, name, &items[1..], out);
            return;
        }
        if value::symbol_is(head, kw::COND) {
            // (cond test1 res1 test2 res2 … [:else resN]) — flat. Odd offsets
            // (1,3,…) are tests (non-tail); even offsets (2,4,…) are results
            // (inherit `tail`). `:else` sits in a test slot.
            for (i, &child) in items.iter().enumerate().skip(1) {
                walk(heap, child, tail && i % 2 == 0, name, out);
            }
            return;
        }
        if value::symbol_is(head, kw::LET)
            || value::symbol_is(head, kw::LET_STAR)
            || value::symbol_is(head, kw::LETREC)
        {
            // (let (n1 v1 n2 v2 …) body…): binding *values* are non-tail; body
            // is an implicit `do`.
            if let Some(binds) = items.get(1).and_then(|&b| list_items(heap, b)) {
                // values are at odd indices (1,3,5,…) of the flat binding list.
                for v in binds.iter().skip(1).step_by(2) {
                    walk(heap, *v, false, name, out);
                }
            }
            analyze_body(heap, name, &items[2..], out);
            return;
        }
        if value::symbol_is(head, kw::DEF) || value::symbol_is(head, kw::DEFMACRO) {
            // A nested definition: its value expression is non-tail.
            if let Some(&v) = items.get(2) {
                walk(heap, v, false, name, out);
            }
            return;
        }

        // A regular call. If it's a self-call and we're NOT in tail position,
        // flag it. Either way the arguments are evaluated in non-tail position.
        if head == name && !tail {
            out.push((
                heap.form_pos(form),
                format!(
                    "{}: recursive call in non-tail position — deep recursion overflows the \
                     stack; restructure so the self-call is the last thing evaluated \
                     (a tail-recursive accumulator) or drive the loop with a process",
                    value::symbol_name(name)
                ),
            ));
        }
        for &arg in items.iter().skip(1) {
            walk(heap, arg, false, name, out);
        }
    } else {
        // Computed head (e.g. ((fn …) args)) — everything is non-tail.
        for &child in &items {
            walk(heap, child, false, name, out);
        }
    }
}
