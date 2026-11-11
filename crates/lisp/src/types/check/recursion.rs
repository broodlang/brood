//! Non-tail self-recursion lint (advisory), for a `defn` and for a `letrec`-bound `fn`.
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
//! `match`/`case`/threading macros — and `when`/`cond`/`and`/`or`,
//! which are all prelude macros lowering to `if`/`do`/`let` — are already gone,
//! and a `defn` is `(def name (fn …))`. That leaves a small, fixed set of
//! tail-propagating *special forms* to model — `if` / `do` / `let` /
//! `letrec` — mirroring the evaluator's `'tail:` handling. (`def`/`defmacro`
//! survive expansion too and are handled as nested-definition cases.)
//! **Conservative by design:** it descends only into forms
//! whose tail structure it knows, stops at nested `fn`/`lambda` (a self-call
//! inside an inner closure is a different frame) and at `quote`/`quasiquote`
//! (data), and only flags a self-call it is *certain* sits in non-tail
//! position. It would rather miss a case than warn on correct code.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Value};
use crate::error::Pos;
use crate::types::check::{Finding, Lint};

use super::walk::list_items;

/// Entry: find every `(def NAME (fn …))` anywhere in `form` and check each for
/// self-calls to `NAME` in non-tail position.
pub(super) fn check_recursion(heap: &Heap, form: Value, out: &mut Vec<Finding>) {
    // Deep-form stack safety — the same stacker remedy as walk.rs's
    // check_into/collect_def_names.
    let before = out.len();
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        check_recursion_inner(heap, form, out)
    });
    // One fault, one diagnostic. A pattern-clause `defn` is lowered to `%match`
    // before we see it, and the lowering repeats the body across dispatch paths, so
    // a single non-tail self-call was walked — and reported — three times over. The
    // duplicates are indistinguishable by position and text, which is exactly what
    // makes them safe to collapse here rather than in every consumer.
    let mut seen: std::collections::HashSet<Finding> = std::collections::HashSet::new();
    let deduped: Vec<Finding> = out
        .split_off(before)
        .into_iter()
        .filter(|finding| seen.insert(finding.clone()))
        .collect();
    out.extend(deduped);
}

fn check_recursion_inner(heap: &Heap, form: Value, out: &mut Vec<Finding>) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    // `(%lint-allow :non-tail-recursion body…)` — the `check-allow` suppression
    // marker (see `walk.rs`). When it names *this* lint, skip its whole subtree:
    // the wrapped defs deliberately recurse non-tail (JIT depth-cliff torture
    // tests) and the author has opted out. A marker for a *different* category
    // falls through and is walked normally.
    if let Some(&Value::Keyword(cat)) = items.get(1)
        && matches!(items.first(), Some(&Value::Sym(h)) if value::symbol_is(h, "%lint-allow"))
        && value::symbol_is(cat, "non-tail-recursion")
    {
        return;
    }
    // Quoted data is not code. `assert=` quotes the form it checks into its failure
    // message, so a definition under test was walked twice: once expanded, and once as
    // the quoted copy, which is NOT expanded — a `match` there is an ordinary call, so
    // the self-call in its last clause read as an argument, in non-tail position.
    if matches!(items.first(), Some(&Value::Sym(h))
        if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE))
    {
        return;
    }
    if let (Some(&Value::Sym(head)), true) = (items.first(), items.len() >= 3)
        && value::symbol_is(head, kw::DEF)
        && let Value::Sym(name) = items[1]
        && is_fn_form(heap, items[2])
    {
        analyze_fn(heap, name, items[2], heap.form_pos_only(form), out);
    }
    // A `letrec` loop is the other place a self-call lives: `(letrec (go (fn (i) …)) (go
    // 0))` is how a body loops without a top-level `defn`, and its self-call is held to the
    // same rule. Each `fn`-valued binding is analysed under its LOCAL name; a call to a
    // sibling is not a self-call (mutual recursion is out of scope here, as for `defn`).
    if let (Some(&Value::Sym(head)), true) = (items.first(), items.len() >= 3)
        && value::symbol_is(head, kw::LETREC)
        && let Some(binds) = list_items(heap, items[1])
    {
        for pair in binds.chunks(2) {
            if let [Value::Sym(name), bound] = *pair
                && is_fn_form(heap, bound)
            {
                analyze_fn(heap, name, bound, heap.form_pos_only(form), out);
            }
        }
    }
    // Recurse so nested definitions (a `defn` inside `do` / `test` / `describe`,
    // a closure-defining `def` in a body) are covered too.
    for &child in &items {
        check_recursion(heap, child, out);
    }
}

/// Is `v` a `(fn …)` form?
fn is_fn_form(heap: &Heap, v: Value) -> bool {
    matches!(list_items(heap, v).as_deref(),
        Some([Value::Sym(h), ..]) if value::symbol_is(*h, kw::FN))
}

/// Analyze a `(fn …)` value: one body for a single-arity fn, or each arm's body
/// for a multi-arity fn. The fn body's last form is in tail position.
fn analyze_fn(
    heap: &Heap,
    name: Symbol,
    fnval: Value,
    enclosing: Option<Pos>,
    out: &mut Vec<Finding>,
) {
    let Some(items) = list_items(heap, fnval) else {
        return;
    };
    // A docstring rides along as the element after `fn` — `(defn f "doc" ((a) …) ((a b)
    // …))` expands to `(fn "doc" ((a) …) ((a b) …))`. Skip it, or a docstring'd multi-arity
    // fn reads as SINGLE-arity — its arms the "body" — and an arm delegating to another
    // arm is flagged as non-tail recursion.
    let items: Vec<Value> = match items.get(1) {
        Some(Value::Str(_)) if items.len() > 2 => {
            let mut v = vec![items[0]];
            v.extend_from_slice(&items[2..]);
            v
        }
        _ => items.to_vec(),
    };
    // items = [fn, params-or-arm, ...]. Multi-arity iff the element after `fn`
    // is a clause `((params) body…)` — i.e. its own first element is a list.
    // (Pattern clauses are already lowered to `%match`, so post-expansion arms
    // are arity-only: their params are a plain symbol list.)
    let multi = matches!(items.get(1), Some(&arm) if first_is_list(heap, arm));
    if multi {
        for &arm in &items[1..] {
            // arm = (params body…): body is everything after the param list.
            if let Some(arm_items) = list_items(heap, arm)
                && !binds_symbol(heap, arm_items[0], name)
            {
                analyze_body(heap, name, &arm_items[1..], enclosing, out);
            }
        }
    } else {
        // (fn params body…): body is everything after the param list. A bare
        // `(fn)` — mid-edit source; the advisory checker sees anything — has
        // neither params nor body, and must not panic the checker thread.
        // A parameter spelled like the function shadows it: `(go)` in the body calls the
        // argument, not the function.
        if let Some(body) = items.get(2..)
            && !binds_symbol(heap, items[1], name)
        {
            analyze_body(heap, name, body, enclosing, out);
        }
    }
}

/// True if `v` is a list whose first element is itself a list (the multi-arity
/// clause shape). A single-arity param list's first element is a symbol or a
/// vector pattern, never a bare list.
fn first_is_list(heap: &Heap, v: Value) -> bool {
    // A zero-arity arm's parameter list is the empty list, which is `nil`: `(() (loop1 0))`.
    matches!(list_items(heap, v).as_deref(),
        Some([first, ..]) if matches!(first, Value::Pair(_) | Value::Nil))
}

/// Does the binding target `target` — a symbol, or a destructuring pattern — bind `name`?
fn binds_symbol(heap: &Heap, target: Value, name: Symbol) -> bool {
    match target {
        Value::Sym(symbol) => symbol == name,
        Value::Pair(_) => list_items(heap, target)
            .unwrap_or_default()
            .into_iter()
            .any(|part| binds_symbol(heap, part, name)),
        Value::Vector(id) => heap
            .vector(id)
            .iter()
            .any(|&part| binds_symbol(heap, part, name)),
        Value::Map(id) => heap
            .map_entries(id)
            .into_iter()
            .any(|(key, part)| binds_symbol(heap, key, name) || binds_symbol(heap, part, name)),
        _ => false,
    }
}

/// A body (implicit `do`): the last form is in tail position, the rest are not.
fn analyze_body(
    heap: &Heap,
    name: Symbol,
    body: &[Value],
    enclosing: Option<Pos>,
    out: &mut Vec<Finding>,
) {
    let here = enclosing;
    if body.is_empty() {
        return;
    }
    let last = body.len() - 1;
    for (i, &form) in body.iter().enumerate() {
        walk(heap, form, i == last, name, here, out);
    }
}

/// Walk `form`, knowing whether it is in tail position, flagging any non-tail
/// call to `name`.
///
/// Grows the stack in heap-backed segments, like every other walker here — this is
/// the recursion that descends a function's body, so a deeply-nested body inside a
/// `(def n (fn …))` would otherwise abort the process.
fn walk(
    heap: &Heap,
    form: Value,
    tail: bool,
    name: Symbol,
    enclosing: Option<Pos>,
    out: &mut Vec<Finding>,
) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        walk_inner(heap, form, tail, name, enclosing, out)
    })
}

fn walk_inner(
    heap: &Heap,
    form: Value,
    tail: bool,
    name: Symbol,
    enclosing: Option<Pos>,
    out: &mut Vec<Finding>,
) {
    // the nearest known source position: this form's own, else the one handed down
    let here = heap.form_pos_only(form).or(enclosing);
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
        {
            return;
        }
        // Tail-propagating special forms — mirror the evaluator's `'tail:` rules.
        if value::symbol_is(head, kw::IF) {
            // (if test then else?): test is non-tail; then/else inherit `tail`.
            if let Some(&t) = items.get(1) {
                walk(heap, t, false, name, here, out);
            }
            for &branch in items.iter().skip(2) {
                walk(heap, branch, tail, name, here, out);
            }
            return;
        }
        if value::symbol_is(head, kw::DO) {
            // do: the last form is tail; earlier forms are for effect (non-tail).
            analyze_body(heap, name, &items[1..], here, out);
            return;
        }
        if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
            // (let (n1 v1 n2 v2 …) body…): binding *values* are non-tail; body
            // is an implicit `do`.
            //
            // A binding of `name` itself shadows the function from there on (`(let (step
            // (fn …)) (step x))` inside `(defn step …)` calls the LOCAL): stop at it. A
            // `letrec` binding is in scope in every value, so one shadows them all.
            let binds = items
                .get(1)
                .and_then(|&b| list_items(heap, b))
                .unwrap_or_default();
            let recursive = value::symbol_is(head, kw::LETREC);
            let pairs: Vec<&[Value]> = binds.chunks(2).collect();
            if recursive && pairs.iter().any(|pair| binds_symbol(heap, pair[0], name)) {
                return;
            }
            for pair in pairs {
                // values are the second element of each flat binding pair.
                if let Some(&bound_value) = pair.get(1) {
                    walk(heap, bound_value, false, name, here, out);
                }
                if binds_symbol(heap, pair[0], name) {
                    return;
                }
            }
            analyze_body(heap, name, &items[2..], here, out);
            return;
        }
        if value::symbol_is(head, kw::DEF) || value::symbol_is(head, kw::DEFMACRO) {
            // A nested definition: its value expression is non-tail.
            if let Some(&v) = items.get(2) {
                walk(heap, v, false, name, here, out);
            }
            return;
        }

        // A regular call. If it's a self-call and we're NOT in tail position,
        // flag it. Either way the arguments are evaluated in non-tail position.
        if head == name && !tail {
            out.push(Finding::new(
                Lint::NonTailRecursion,
                // The lowering that turns pattern clauses into `%match` builds fresh
                // forms, and a fresh form carries no source position — so this came
                // out as `line: nil`, which no consumer can place. Fall back to the
                // nearest ENCLOSING form that does have one: the reader is pointed at
                // the definition rather than at nothing.
                heap.form_pos_only(form).or(enclosing),
                format!(
                    "{}: recursive call in non-tail position — deep recursion overflows the \
                     stack; restructure so the self-call is the last thing evaluated \
                     (a tail-recursive accumulator) or drive the loop with a process",
                    value::symbol_name(name)
                ),
            ));
        }
        for &arg in items.iter().skip(1) {
            walk(heap, arg, false, name, here, out);
        }
    } else {
        // Computed head (e.g. ((fn …) args)) — everything is non-tail.
        for &child in &items {
            walk(heap, child, false, name, here, out);
        }
    }
}
