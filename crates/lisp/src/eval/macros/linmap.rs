//! The linear-map rewrite: a fold that only threads one map through updates runs over
//! a table, behind a seed check that falls back to the form as written.

use super::*;

/// Wrapper-split a **linear immutable-map fold** (ADR-112, ADR-360 in docs/decisions.md).
/// Given a fully-expanded `(def NAME (fn (P…) BODY…))` whose accumulator is provably
/// linear (checked by `linmap_probe`), rewrite it to
///
/// ```text
/// (do (%lint-allow :generated (def SLOW (fn (P…) BODY[NAME→SLOW])))  ; the loop as written
///     (def INNER (fn (P…) BODY'))                  ; in-place table loop
///     (def NAME  (fn (P…) (let (t (%table-from-map ACC))
///                           (if t (table-snapshot (INNER … t …)) (SLOW …))))))
/// ```
///
/// where `BODY'` swaps the self-recursion head `NAME→INNER` and every whitelisted
/// map op on the accumulator to its in-place `table-*` op. The inner loop builds a
/// private table (seeded once by the wrapper, frozen once on return), so the
/// per-update path-copy is gone with no per-iteration cost. `%table-from-map` answers
/// `nil` for a seed a table cannot stand in for — a value a table cannot hold (a rope,
/// a builtin, a lazy view), or a record, whose misses consult `Lookup` — and the wrapper
/// then runs `SLOW`, the loop exactly as the author wrote it (self-recursion re-pointed
/// at itself, nothing else touched). Without it a tally seeded with `{:r <rope>}` would
/// raise `cannot send a rope` only with the rewrite on; the rewrite has to be
/// unobservable on EVERY input, and the seed is
/// the one thing the linearity proof cannot see. Returns the replacement, or `None` to
/// leave the `def` unchanged. `items` is the expanded `def` form.
pub(super) fn linmap_split_def(heap: &mut Heap, items: &[Value]) -> Option<Value> {
    if items.len() != 3 {
        return None;
    }
    if !matches!(items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::DEF)) {
        return None;
    }
    let name_sym = match items[1].unpack() {
        ValueRef::Sym(s) => s,
        _ => return None,
    };
    let fn_items = heap.list_items(items[2])?;
    if fn_items.len() < 3
        || !matches!(fn_items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN))
    {
        return None;
    }
    let param_form = fn_items[1];
    let param_vals = heap.list_or_vector_items(param_form)?;
    let mut params: Vec<value::Symbol> = Vec::with_capacity(param_vals.len());
    for &pv in &param_vals {
        match pv.unpack() {
            ValueRef::Sym(s)
                if !value::symbol_is(s, kw::AMP) && !value::symbol_is(s, kw::AMP_OPTIONAL) =>
            {
                params.push(s)
            }
            _ => return None, // &optional / & rest / pattern param → bail
        }
    }
    let body = &fn_items[2..];
    // A leading docstring complicates the split; skip it conservatively for now.
    if body.len() > 1 && matches!(body[0].unpack(), ValueRef::Str(_)) {
        return None;
    }
    // A `quasiquote` anywhere in the body declines the split. The probe runs on the *Node*
    // IR, where an unquote's contents are ordinary code, so an `acc` use inside one counts
    // toward the linearity proof — but `linmap_rewrite_form` cannot safely rewrite inside a
    // quasiquote (it would have to distinguish quoted structure from unquoted code), and
    // leaving it alone would let a `%map-get` reach the in-place `Table`. Plain `quote` needs
    // no gate: it is inert data the probe sees as a `Const`, and the rewriter now passes it
    // through untouched.
    if body.iter().any(|&f| form_has_quasiquote(heap, f, 0)) {
        return None;
    }
    // Soundness gate: reuse the Node-level reachability analysis as a probe.
    let idx = crate::eval::compile::linmap_probe(heap, name_sym, &params, body)?;

    let inner_val = value::gensym(&format!("{}/linmap-loop", value::symbol_name(name_sym)));
    let inner_body: Vec<Value> = body
        .iter()
        .map(|&f| linmap_rewrite_form(heap, f, name_sym, inner_val, params[idx]))
        .collect();
    // The slow copy keeps the parameter list as written; the inner loop and the wrapper
    // each get their own (`copy_form`).
    let inner_params = copy_form(heap, param_form);
    let mut inner_fn = vec![value::sym(kw::FN), inner_params];
    inner_fn.extend(inner_body);
    let inner_fn = heap.list(inner_fn);
    let inner_def = heap.list(vec![value::sym(kw::DEF), inner_val, inner_fn]);

    // The loop as written, for a seed the table cannot stand in for.
    let slow_val = value::gensym(&format!("{}/linmap-slow", value::symbol_name(name_sym)));
    let slow_body: Vec<Value> = body
        .iter()
        .map(|&f| linmap_rename_self(heap, f, name_sym, slow_val))
        .collect();
    let mut slow_fn = vec![value::sym(kw::FN), param_form];
    slow_fn.extend(slow_body);
    let slow_fn = heap.list(slow_fn);
    let slow_def = heap.list(vec![value::sym(kw::DEF), slow_val, slow_fn]);
    // The checker walks the inner loop (the author's code, rewritten ops aside) and does
    // not need this copy of it: `:generated` skips the subtree outright.
    let slow_def = heap.list(vec![
        value::sym("%lint-allow"),
        value::kw("generated"),
        slow_def,
    ]);

    let t_val = value::gensym("linmap-table");
    let mut call = vec![inner_val];
    let mut slow_call = vec![slow_val];
    for (i, &pv) in param_vals.iter().enumerate() {
        call.push(if i == idx { t_val } else { pv });
        slow_call.push(pv);
    }
    let inner_call = heap.list(call);
    let slow_call = heap.list(slow_call);
    // Snapshot the loop's result back to an immutable map **only when it is the table**.
    // The accumulator is what the linearity proof licenses rewriting in place, but the base
    // case is free to return something else entirely — `(%map-count acc)`, `(%map-get acc :a)`,
    // or a plain constant — and an unconditional `table-snapshot` then failed on a value it
    // was never given: `table-snapshot: expected table, got int (3)`. `linmap_linear` admits
    // those returns deliberately (a whitelisted *read* of the accumulator, or a `Const`), so
    // the wrapper, not the proof, is what has to account for them.
    //
    // `(= :table (type-of r))` rather than a predicate call: `type-of` is a total PrimOp1
    // (never deopts) and the comparison is against an interned keyword, so this costs one
    // tag read on the way out of a fold — and only on the way out, not per element.
    let r_val = value::gensym("linmap-out");
    let type_of = heap.list(vec![value::sym("/type-of"), r_val]);
    let is_table = heap.list(vec![value::sym("/="), value::kw("table"), type_of]);
    let snap_call = heap.list(vec![value::sym(kw::TABLE_SNAPSHOT), r_val]);
    let cond = heap.list(vec![value::sym(kw::IF), is_table, snap_call, r_val]);
    // Then free the private table: its loop has returned, so nothing holds it, and
    // without this every call kept a table's shell and its dense reservation for good —
    // a tally called ~25 000 times exhausted a 16 GB address-space cap.
    let cond = heap.list(vec![value::sym(kw::TABLE_FREE), t_val, cond]);
    let bind = heap.list(vec![r_val, inner_call]);
    let snap = heap.list(vec![value::sym(kw::LET), bind, cond]);
    // (let (t (%table-from-map ACC)) (if t <snap> (SLOW …)))
    let seed = heap.list(vec![value::sym("%table-from-map"), param_vals[idx]]);
    let choose = heap.list(vec![value::sym(kw::IF), t_val, snap, slow_call]);
    let seed_bind = heap.list(vec![t_val, seed]);
    let wrapper_body = heap.list(vec![value::sym(kw::LET), seed_bind, choose]);
    let wrapper_params = copy_form(heap, param_form);
    let wrapper_fn = heap.list(vec![value::sym(kw::FN), wrapper_params, wrapper_body]);
    let wrapper_def = heap.list(vec![value::sym(kw::DEF), items[1], wrapper_fn]);
    Some(heap.list(vec![value::sym(kw::DO), slow_def, inner_def, wrapper_def]))
}

/// The names the fold rewrite reads by spelling on the expanded form: a local binder of
/// any of them anywhere in the top-level form declines the rewrite for the whole form. The
/// defn split needs no such scan — its probe compiles the body, where a shadowed `get` is
/// a `Local` callee and an escape — but a `fold` shadowed OUTSIDE the literal is invisible
/// to a probe of the literal alone.
const LINMAP_FOLD_HEADS: [&str; 8] = ["fold", "reduce", "get", "assoc", "+", "-", "inc", "dec"];

/// [`LINMAP_FOLD_HEADS`] interned, once per process (symbols are process-global).
static LINMAP_FOLD_NAMES: std::sync::LazyLock<Vec<value::Symbol>> =
    std::sync::LazyLock::new(|| LINMAP_FOLD_HEADS.iter().map(|n| value::intern(n)).collect());

/// Does `form` bind any of `names` — a `let`/`letrec` target, a `fn` parameter (single or
/// multi-clause)? Post-expansion, so binders are plain symbols. Quoted data is skipped.
pub(super) fn form_binds_any(
    heap: &Heap,
    form: Value,
    names: &[value::Symbol],
    depth: usize,
) -> bool {
    if depth > 256 {
        return true; // too deep to prove clean — decline
    }
    let Some(items) = heap.list_items(form) else {
        // A literal's elements are code too: `[(let (x 1) x)]` binds `x`.
        return literal_items(heap, form).is_some_and(|xs| {
            xs.iter()
                .any(|&x| form_binds_any(heap, x, names, depth + 1))
        });
    };
    let Some(&head) = items.first() else {
        return false;
    };
    let binds = |v: Value| matches!(v.unpack(), ValueRef::Sym(s) if names.contains(&s));
    if let ValueRef::Sym(h) = head.unpack() {
        if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) {
            return false;
        }
        if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
            if let Some(bs) = items.get(1).and_then(|&b| heap.list_or_vector_items(b))
                && bs.iter().step_by(2).any(|&b| binds(b))
            {
                return true;
            }
        } else if value::symbol_is(h, kw::FN) {
            // `(fn (params) …)`, or `(fn ((params) …) ((params) …))`.
            let clauses: Vec<Value> = if fn_is_arity_multi_clause(heap, &items) {
                items[1..]
                    .iter()
                    .filter_map(|&c| {
                        heap.list_or_vector_items(c)
                            .and_then(|v| v.first().copied())
                    })
                    .collect()
            } else {
                items.get(1).copied().into_iter().collect()
            };
            for c in clauses {
                if let Some(ps) = heap.list_or_vector_items(c)
                    && ps.iter().any(|&p| binds(p))
                {
                    return true;
                }
            }
        }
    }
    items
        .iter()
        .any(|&it| form_binds_any(heap, it, names, depth + 1))
}

/// Rewrite every `(fold COLL INIT (fn (acc x…) BODY…))` / `reduce` in the expanded top-level
/// `form` whose literal is a linear tally of `acc` (`linmap_probe_fn`) into an in-place build:
///
/// ```text
/// (let (C COLL  I INIT  T (%table-from-map I))
///   (if T
///       (let (R (fold C T (fn (acc x…) BODY')))
///         (if (= :table (type-of R)) (%table-snapshot R) R))
///       (%lint-allow :generated (fold C I (fn (acc x…) BODY…)))))
/// ```
///
/// `BODY'` is `BODY` with the accumulator's ops on the table (`linmap_rewrite_form` with no
/// self-name to re-point). `COLL` and `INIT` are bound first, in the source's order, so each
/// is evaluated once; the seed check and the unrewritten copy are the defn split's
/// (`linmap_split_def`). Declined for the whole form when any binder in it shadows a name
/// the rewrite reads (`form_binds_any`). A form already rewritten is not rewritten again:
/// the walk does not enter `(%lint-allow :generated …)`, and the table-bound copy no longer
/// has the shape (its accumulator's ops are table ops, which the probe treats as escapes).
pub(super) fn linmap_fold_rewrite(heap: &mut Heap, form: Value) -> Value {
    if !form_has_fold_literal(heap, form) || form_binds_any(heap, form, &LINMAP_FOLD_NAMES, 0) {
        return form;
    }
    pipeline_walk(heap, form, 0, &linmap_fold_call)
}

/// Is there a `(fold … (fn …))` / `reduce` anywhere in `form`? The cheap pre-check that
/// keeps the binder scan and the rebuilding walk off every ordinary top-level form.
fn form_has_fold_literal(heap: &Heap, form: Value) -> bool {
    form_has_call(heap, form, 0, &|head, items| {
        is_fold_head(head)
            && items.len() == 4
            && matches!(heap.list_items(items[3]).and_then(|f| f.first().map(|v| v.unpack())),
                Some(ValueRef::Sym(f)) if value::symbol_is(f, kw::FN))
    })
}

/// The rewrite of one `(fold COLL INIT (fn (acc x…) BODY…))` call, or `None` when `items` is
/// not that shape or the literal is not a linear tally.
fn linmap_fold_call(heap: &mut Heap, form: Value, items: &[Value]) -> Option<Value> {
    let ValueRef::Sym(h) = items[0].unpack() else {
        return None;
    };
    if !is_fold_head(h) || items.len() != 4 {
        return None;
    }
    let fn_items = heap.list_items(items[3])?;
    if fn_items.len() < 3
        || !matches!(fn_items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN))
        || fn_is_arity_multi_clause(heap, &fn_items)
    {
        return None;
    }
    let param_form = fn_items[1];
    let param_vals = heap.list_or_vector_items(param_form)?;
    let mut params: Vec<value::Symbol> = Vec::with_capacity(param_vals.len());
    for &pv in &param_vals {
        match pv.unpack() {
            ValueRef::Sym(s)
                if !value::symbol_is(s, kw::AMP) && !value::symbol_is(s, kw::AMP_OPTIONAL) =>
            {
                params.push(s)
            }
            _ => return None,
        }
    }
    if params.is_empty() {
        return None;
    }
    let body = &fn_items[2..];
    if body.len() > 1 && matches!(body[0].unpack(), ValueRef::Str(_)) {
        return None;
    }
    if body.iter().any(|&f| form_has_quasiquote(heap, f, 0)) {
        return None;
    }
    crate::eval::compile::linmap_probe_fn(heap, &params, body)?;
    let acc = params[0];
    // The literal's own name is nothing: no self-call to re-point. Any symbol not in the
    // body serves as the "name"; the literal's accumulator symbol cannot be a head there.
    let no_name = value::gensym("linmap-fold");
    let ValueRef::Sym(no_name_sym) = no_name.unpack() else {
        return None;
    };
    let fast_body: Vec<Value> = body
        .iter()
        .map(|&f| linmap_rewrite_form(heap, f, no_name_sym, no_name, acc))
        .collect();
    // The unrewritten literal keeps its parameter list; the fast one gets a copy.
    let fast_params = copy_form(heap, param_form);
    let mut fast_fn = vec![value::sym(kw::FN), fast_params];
    fast_fn.extend(fast_body);
    let fast_fn = rebuild_list(heap, items[3], fast_fn);

    let c_val = value::gensym("linmap-coll");
    let i_val = value::gensym("linmap-init");
    let t_val = value::gensym("linmap-table");
    let r_val = value::gensym("linmap-out");
    let fast_call = rebuild_list(heap, form, vec![items[0], c_val, t_val, fast_fn]);
    let type_of = heap.list(vec![value::sym("/type-of"), r_val]);
    let is_table = heap.list(vec![value::sym("/="), value::kw("table"), type_of]);
    let snap_call = heap.list(vec![value::sym(kw::TABLE_SNAPSHOT), r_val]);
    let cond = heap.list(vec![value::sym(kw::IF), is_table, snap_call, r_val]);
    // Free the private table once the fold has returned — see `linmap_split_def`.
    let cond = heap.list(vec![value::sym(kw::TABLE_FREE), t_val, cond]);
    let bind = heap.list(vec![r_val, fast_call]);
    let snap = heap.list(vec![value::sym(kw::LET), bind, cond]);
    let slow_call = rebuild_list(heap, form, vec![items[0], c_val, i_val, items[3]]);
    let slow = heap.list(vec![
        value::sym("%lint-allow"),
        value::kw("generated"),
        slow_call,
    ]);
    let choose = heap.list(vec![value::sym(kw::IF), t_val, snap, slow]);
    let seed = heap.list(vec![value::sym("%table-from-map"), i_val]);
    let binds = heap.list(vec![c_val, items[1], i_val, items[2], t_val, seed]);
    Some(rebuild_list(
        heap,
        form,
        vec![value::sym(kw::LET), binds, choose],
    ))
}

/// `form` with every self-call head `name` re-pointed at `slow` and nothing else touched —
/// the unrewritten copy of a linmap loop keeps recursing into itself rather than back
/// through the wrapper (which would re-run the seed check per iteration). Quoted data is
/// left alone, as in `linmap_rewrite_form`.
fn linmap_rename_self(heap: &mut Heap, form: Value, name: value::Symbol, slow: Value) -> Value {
    let items = match form.unpack() {
        ValueRef::Pair(_) => match heap.list_items(form) {
            Some(v) if !v.is_empty() => v,
            _ => return copy_form(heap, form),
        },
        // A literal (a vector of code) is copied too: the rewritten loop holds the other
        // copy of this body, and a form is in one position (`copy_form`).
        _ => return copy_form(heap, form),
    };
    if let ValueRef::Sym(h) = items[0].unpack() {
        if value::symbol_is(h, kw::QUOTE) {
            return copy_form(heap, form);
        }
        if h == name {
            let mut c = vec![slow];
            for &a in &items[1..] {
                c.push(linmap_rename_self(heap, a, name, slow));
            }
            return rebuild_list(heap, form, c);
        }
    }
    let out: Vec<Value> = items
        .iter()
        .map(|&it| linmap_rename_self(heap, it, name, slow))
        .collect();
    rebuild_list(heap, form, out)
}

/// The `E` of `(+ (get ACC KEY 0) E)` / `(+ E (get ACC KEY 0))` / `(- (get ACC KEY 0) E)`,
/// or the `1` of `(inc (get ACC KEY 0))` / `dec`, with whether it subtracts — the source
/// half of `LinIdiom::fused_add`, which decided on the compiled body that this `assoc` is
/// the tally. The two keys must be the same form — the probe's
/// `linmap_same_key` is the stricter judge (it also requires the form to be pure), and
/// the rewrite only ever runs on a body the probe admitted.
fn linmap_fused_addend(
    heap: &Heap,
    acc: value::Symbol,
    key: Value,
    value: Value,
) -> Option<(Value, bool)> {
    let same_key = |k: Value| heap.equal(key, k);
    let is_get_key_0 = |form: Value| {
        let Some(g) = heap.list_items(form) else {
            return false;
        };
        g.len() == 4
            && matches!(g[0].unpack(), ValueRef::Sym(h) if value::symbol_is(h, "get"))
            && matches!(g[1].unpack(), ValueRef::Sym(a) if a == acc)
            && same_key(g[2])
            && matches!(g[3].unpack(), ValueRef::Int(0))
    };
    let items = heap.list_items(value)?;
    let ValueRef::Sym(h) = items.first()?.unpack() else {
        return None;
    };
    // `(inc (get acc K 0))` / `(dec …)`: the addend is 1.
    if items.len() == 2 && is_get_key_0(items[1]) {
        if value::symbol_is(h, "inc") {
            return Some((Value::int(1), false));
        }
        if value::symbol_is(h, "dec") {
            return Some((Value::int(1), true));
        }
        return None;
    }
    if items.len() != 3 {
        return None;
    }
    if value::symbol_is(h, "+") {
        if is_get_key_0(items[1]) {
            return Some((items[2], false));
        }
        if is_get_key_0(items[2]) {
            return Some((items[1], false));
        }
    } else if value::symbol_is(h, "-") && is_get_key_0(items[1]) {
        return Some((items[2], true));
    }
    None
}

/// Does `form` contain a `quasiquote` anywhere? Guards the linmap wrapper-split (see the
/// call site). Depth-bounded so a pathological datum can't recurse the compiler thread.
pub(super) fn form_has_quasiquote(heap: &Heap, form: Value, depth: usize) -> bool {
    if depth > 64 {
        return true; // too deep to prove clean — decline the split
    }
    match heap.list_items(form) {
        Some(items) => items.iter().enumerate().any(|(i, &it)| {
            (i == 0
                && matches!(it.unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::QUASIQUOTE)))
                || form_has_quasiquote(heap, it, depth + 1)
        }),
        None => literal_items(heap, form)
            .is_some_and(|xs| xs.iter().any(|&x| form_has_quasiquote(heap, x, depth + 1))),
    }
}

/// The elements of a vector, map (keys and values) or set literal — code a walk over a form
/// must enter as it enters a list's items, since the literal evaluates each one. `None` for
/// anything else.
pub(super) fn literal_items(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form.unpack() {
        ValueRef::Vector(id) => Some(heap.vector(id).to_vec()),
        ValueRef::Map(id) => Some(
            heap.map_entries(id)
                .into_iter()
                .flat_map(|(k, v)| [k, v])
                .collect(),
        ),
        ValueRef::Set(id) => Some(heap.set_elems(id)),
        _ => None,
    }
}

/// Source rewrite for the inner loop of a wrapper-split: turn the self-recursion
/// head `name → inner`, and each whitelisted map op whose first arg is the
/// accumulator `acc` into its in-place table op (updates yield the table via a
/// `do`; reads pass straight through). The `%`-op mapping is `linmap_update_op` /
/// `linmap_read_op`, the probe's own whitelist; the idiom spellings below must stay in
/// lockstep with `LinIdiom`.
fn linmap_rewrite_form(
    heap: &mut Heap,
    form: Value,
    name: value::Symbol,
    inner: Value,
    acc: value::Symbol,
) -> Value {
    let items = match form.unpack() {
        ValueRef::Pair(_) => match heap.list_items(form) {
            Some(v) if !v.is_empty() => v,
            _ => return copy_form(heap, form),
        },
        // An atom: symbols (incl. a bare `acc` return) pass through; a vector, map or set
        // literal is copied, since the unrewritten copy holds the original (`copy_form`).
        _ => return copy_form(heap, form),
    };
    if let ValueRef::Sym(h) = items[0].unpack() {
        let second_is_acc =
            matches!(items.get(1).map(|v| v.unpack()), Some(ValueRef::Sym(s)) if s == acc);
        if second_is_acc {
            // The probe's own op tables, so the rewrite maps exactly the ops it admits.
            if let Some(t) = crate::eval::compile::linmap_update_op(h) {
                // (op acc rest…) → (do (table-op acc rest…) acc) — mutate, yield table.
                let mut c = vec![value::sym(t), items[1]];
                for &a in &items[2..] {
                    c.push(linmap_rewrite_form(heap, a, name, inner, acc));
                }
                let mutate = rebuild_list(heap, form, c);
                return rebuild_list(heap, form, vec![value::sym(kw::DO), mutate, items[1]]);
            }
            if let Some(t) = crate::eval::compile::linmap_read_op(h) {
                let mut c = vec![value::sym(t), items[1]];
                for &a in &items[2..] {
                    c.push(linmap_rewrite_form(heap, a, name, inner, acc));
                }
                return rebuild_list(heap, form, c);
            }
            // The idiomatic spellings the probe admits through `LinIdiom` (inline.rs):
            // `(get acc k [d])` is the read `(%table-get acc k d)`, and the tally
            // `(assoc acc K (+ (get acc K 0) E))` — either operand order — is the fused
            // `(%table-add acc K E)`. `%table-add` adds exactly as `+` does, so the
            // float/bignum/error behaviour of the source is kept, not just the int case.
            if value::symbol_is(h, "get") && (items.len() == 3 || items.len() == 4) {
                let key = linmap_rewrite_form(heap, items[2], name, inner, acc);
                let default = match items.get(3) {
                    Some(&d) => linmap_rewrite_form(heap, d, name, inner, acc),
                    None => Value::nil(),
                };
                return rebuild_list(
                    heap,
                    form,
                    vec![value::sym(kw::TABLE_GET), items[1], key, default],
                );
            }
            if value::symbol_is(h, "assoc")
                && items.len() == 4
                && let Some((addend, sub)) = linmap_fused_addend(heap, acc, items[2], items[3])
            {
                let key = linmap_rewrite_form(heap, items[2], name, inner, acc);
                let addend = linmap_rewrite_form(heap, addend, name, inner, acc);
                let op = if sub { kw::TABLE_SUB } else { kw::TABLE_ADD };
                let mutate = rebuild_list(heap, form, vec![value::sym(op), items[1], key, addend]);
                return rebuild_list(heap, form, vec![value::sym(kw::DO), mutate, items[1]]);
            }
        }
        if h == name {
            // Self-recursion → call the inner loop instead.
            let mut c = vec![inner];
            for &a in &items[1..] {
                c.push(linmap_rewrite_form(heap, a, name, inner, acc));
            }
            return rebuild_list(heap, form, c);
        }
        // `(quote …)` is inert DATA, never evaluated — rewriting inside it corrupts the
        // datum instead of the program. `(io/puts '(%map-get acc 1))` printed
        // `(table-get acc 1)`. The probe cannot catch this: a quoted form compiles to a
        // single `Node::Const`, so the linearity analysis sees no `acc` use at all and
        // passes, while the source rewrite walks straight through the quote.
        // Copied, not returned: the unrewritten copy holds this very form (`copy_form`).
        if value::symbol_is(h, kw::QUOTE) {
            return copy_form(heap, form);
        }
    }
    let out: Vec<Value> = items
        .iter()
        .map(|&it| linmap_rewrite_form(heap, it, name, inner, acc))
        .collect();
    rebuild_list(heap, form, out)
}
