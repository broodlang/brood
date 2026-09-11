//! Ability impl return checking: an `impl`'s method bodies against the op's declared
//! `:-> RET` (ADR-180/181).

use super::*;

/// Impl-return conformance: walk the expanded tree for each `(register-impl 'A 'op :id
/// (fn params body…) 'ns)` and, when ability `A`'s op `op` declares a `:-> RET` return
/// type, check the impl's body against it. The gradual relation keeps this
/// false-positive-clean the same way the `sig` return check does — an over-approximated
/// body (a call result) is `dynamic` and defers; only a body *provably disjoint* from the
/// declared return warns. Called after `ctx.set_ability(…)`, so `ctx.ability()` is live.
pub(in crate::types::check) fn check_impl_returns(
    heap: &Heap,
    expanded: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    // Runs for abilities OR multimethods — either kind can declare a return.
    if ctx.ability().is_none_or(|i| i.is_empty()) && ctx.multi().is_none() {
        return;
    }
    for &form in expanded {
        walk_impl_returns(heap, form, ctx, out);
    }
}

pub(super) fn walk_impl_returns(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(head)) = items.first() {
            if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
                return;
            }
            if head_name(head) == kw::REGISTER_IMPL {
                check_one_impl_return(heap, &items, ctx, out);
            }
            if head_name(head) == kw::REGISTER_METHOD {
                check_one_method_return(heap, &items, ctx, out);
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            walk_impl_returns(heap, item, ctx, out);
        }
    })
}

/// Check one `(%register-method 'mname KEY (fn params body…) ns)` against the multimethod's
/// declared `:-> RET`. The multimethod counterpart of [`check_one_impl_return`], and the
/// reason a `defmulti`'s declared return is a **contract** rather than an unchecked
/// assertion: without this, `(defmulti f :-> int)` would let the checker type every call as
/// `int` while a method returned a string, which is worse than not declaring at all.
///
/// Two differences from the impl version. The fn is arg **3** (a method registration carries
/// `mname KEY fn ns`, not `ability op id fn ns`), and there are no declared *parameter*
/// types to seed — a `defmulti` declares only its return, so every param binds unknown. That
/// is a soundness-preserving under-approximation: fewer known types means fewer graded
/// bodies, never a wrong grade.
pub(super) fn check_one_method_return(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(info) = ctx.multi() else {
        return;
    };
    let Some(mname) = items.get(1).and_then(|&v| quoted_sym_name(heap, v)) else {
        return;
    };
    let Some(ret) = info.ret_by_name(&mname) else {
        return;
    };
    // An `any` declared return imposes no constraint — nothing to check.
    if ret.is_any() {
        return;
    }
    let ret = ret.clone();
    // The dispatch key, for the diagnostic: `:default` or a vector of ids.
    // The key arrives quoted in the registration form; unwrap it so the diagnostic reads
    // `for [:string]` rather than `for (quote [:string])`.
    let key = items.get(2).copied().map(|v| {
        let v = list_items(heap, v)
            .filter(|it| {
                it.len() == 2
                    && matches!(it.first(), Some(&Value::Sym(h)) if value::symbol_is(h, kw::QUOTE))
            })
            .and_then(|it| it.get(1).copied())
            .unwrap_or(v);
        match v {
            Value::Keyword(k) => format!(":{}", value::symbol_name(k)),
            other => crate::syntax::printer::print(heap, other),
        }
    });
    let Some(&fn_form) = items.get(3) else {
        return;
    };
    let Some(fn_items) = list_items(heap, fn_form) else {
        return;
    };
    if !matches!(fn_items.first(), Some(&Value::Sym(s)) if is_fn_head(s)) {
        return;
    }
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &fn_items) {
        return;
    }
    let Some(&params_form) = fn_items.get(1) else {
        return;
    };
    // Seed each param from the DISPATCH KEY. A `defmulti` declares only its return, but a
    // method is registered for one concrete key — `[:int :string]` says, positionally, what
    // this method's arguments are, and the runtime will not call it with anything else.
    // Without this every param bound unknown, so `(+ n (string/length s))` widened to
    // `number` and a `:-> int` multimethod could not be satisfied by arithmetic — the same
    // gap an ability impl's `self` had, one level up from rules that were already right.
    //
    // Only names the type lattice knows (`:int`, `:string`, …) seed; a record id or an
    // unrecognised keyword binds unknown, which is the sound under-approximation.
    let key_tys: Vec<Option<Ty>> = items
        .get(2)
        .copied()
        .and_then(|v| {
            let v = list_items(heap, v)
                .filter(|it| {
                    it.len() == 2
                        && matches!(it.first(), Some(&Value::Sym(h)) if value::symbol_is(h, kw::QUOTE))
                })
                .and_then(|it| it.get(1).copied())
                .unwrap_or(v);
            match v {
                Value::Vector(id) => Some(heap.vector(id).to_vec()),
                _ => None,
            }
        })
        .map(|items| {
            items
                .iter()
                .map(|&e| match e {
                    Value::Keyword(k) => crate::types::check::annot::base_ty(&value::symbol_name(k)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let mut scope = ctx.clone();
    for (i, p) in fn_params(heap, params_form).into_iter().enumerate() {
        match key_tys.get(i).and_then(Option::as_ref) {
            Some(ty) => scope = scope.bind_sig_param(p, ty.clone()),
            None => scope = scope.bind(p, None),
        }
    }
    let body_start = match (fn_items.get(2), fn_items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    let Some(&ret_form) = fn_items.get(body_start..).and_then(|b| b.last()) else {
        return;
    };
    let g = gradual_of(heap, ret_form, &scope);
    if !g.consistent_with_mode(ret.clone(), ctx.strict())
        && !ctx.is_suppressed(crate::types::check::ctx::SUPPRESS_TYPE_MISMATCH)
    {
        let key_str = key.map(|k| format!(" for {k}")).unwrap_or_default();
        out.push((
            heap.form_pos_only(ret_form),
            format!(
                "multimethod {mname}{key_str}: declared return type {} but the method yields {}",
                ret, g.bound
            ),
        ));
    }
}

/// Check one `(register-impl 'A 'op :id (fn params body…) 'ns)` against its op's declared
/// return type. The ability + op names come from the two quoted args; the impl fn is
/// arg 4.
pub(super) fn check_one_impl_return(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let info = match ctx.ability() {
        Some(i) => i,
        None => return,
    };
    let (Some(ability), Some(op)) = (
        items.get(1).and_then(|&v| quoted_sym_name(heap, v)),
        items.get(2).and_then(|&v| quoted_sym_name(heap, v)),
    ) else {
        return;
    };
    let Some(ret) = info.op_ret_by_name(&ability, &op) else {
        return;
    };
    // An `any` declared return imposes no constraint — nothing to check.
    if ret.is_any() {
        return;
    }
    let id = items.get(3).copied().and_then(|v| match v {
        Value::Keyword(k) => Some(value::symbol_name(k)),
        _ => None,
    });
    let Some(&fn_form) = items.get(4) else {
        return;
    };
    let Some(fn_items) = list_items(heap, fn_form) else {
        return;
    };
    if !matches!(fn_items.first(), Some(&Value::Sym(s)) if is_fn_head(s)) {
        return;
    }
    // Multi-clause impl fns aren't produced by `impl`; if one appears, don't try to
    // pin a single arity — skip.
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &fn_items) {
        return;
    }
    let Some(&params_form) = fn_items.get(1) else {
        return;
    };
    // Bind the impl's params, seeding each with the op spec's declared `(name T)` type where
    // present (a contract, bound authoritatively like a sig param so the body checks against
    // it) and unknown otherwise, then grade the body's last form against the declared return.
    let param_types = info.op_params_by_name(&ability, &op);
    let mut scope = ctx.clone();
    for (i, p) in fn_params(heap, params_form).into_iter().enumerate() {
        match param_types
            .and_then(|pts| pts.get(i))
            .and_then(Option::as_ref)
        {
            Some(ty) => scope = scope.bind_sig_param(p, ty.clone()),
            // The FIRST param of an impl is the dispatch value, and this impl is registered
            // for one concrete id — so `self` is that record, which the op spec cannot say
            // (it is written once for every implementor, before any of them exist). The
            // record's shape is its constructor's declared return, and the id names the
            // constructor.
            //
            // Without this every `(get self :field)` reads `any`, so `(* (get r :w) (get r :h))`
            // widens to `number` and an op declared `:-> int` can never be satisfied by
            // arithmetic — which is what `ability_test` was reporting.
            None if i == 0 => {
                let seeded = id
                    .as_deref()
                    .and_then(|n| ctx.declared_sig(value::intern(n)))
                    .map(|sig| sig.ret.clone())
                    .filter(|t| t.record_fields().is_some());
                scope = match seeded {
                    Some(t) => scope.bind_sig_param(p, t),
                    None => scope.bind(p, None),
                };
            }
            None => scope = scope.bind(p, None),
        }
    }
    let body_start = match (fn_items.get(2), fn_items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    let Some(&ret_form) = fn_items.get(body_start..).and_then(|b| b.last()) else {
        return;
    };
    let g = gradual_of(heap, ret_form, &scope);
    if !g.consistent_with_mode(ret.clone(), ctx.strict())
        && !ctx.is_suppressed(crate::types::check::ctx::SUPPRESS_TYPE_MISMATCH)
    {
        let id_str = id.map(|i| format!(" for :{i}")).unwrap_or_default();
        out.push((
            heap.form_pos_only(ret_form),
            format!(
                "ability {ability}/{op}{id_str}: declared return type {} but the impl yields {}",
                ret, g.bound
            ),
        ));
    }
}
