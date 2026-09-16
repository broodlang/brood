//! The binder checkers: each clones the enclosing `Ctx`, adds what the form introduces,
//! walks the body in that scope and returns — `fn`, `def`, `defn`, `if` (with guard
//! narrowing) and `let` (with alias propagation).

use super::*;

/// The scope the forms **after** `form` are checked in, when `form` is a guard that
/// **diverges** — `(when (nil? root) (error …))`, which expansion lowers to a branchless
/// `(if COND (error …))`. If the condition held, the body would raise and there would be no
/// "after"; so reaching the next form proves the condition false, exactly as an `if`'s else
/// branch does. `None` when `form` is not that shape.
///
/// This is the language's *early return*. Brood has none — no `return`, no `guard` — so the
/// idiom for "refuse and stop" is a `when` over `error`, and it is everywhere: sixteen call
/// sites in `std/` guard `project/find-root`'s nil this way and then use the value. Without
/// this rule every one of them reads as an unguarded `nil | string`, and the only way to
/// silence that is to declare the producer's return as `any` — which is how it was hidden.
///
/// Sound because the narrowing is the ordinary else-scope of the condition, applied on the
/// path where the then-branch provably cannot have run: `Ty::is_never` is only true for a
/// body that yields nothing, and a body that yields nothing did not fall through.
pub(in crate::types::check) fn diverging_guard_scope(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
) -> Option<Ctx> {
    let items = list_items(heap, form)?;
    let &Value::Sym(head) = items.first()? else {
        return None;
    };
    // Post-expansion. `(when t b)` is `(if t (do b) nil)` and `(unless t b)` is
    // `(if t nil (do b))`, so BOTH arms have to be considered — and a hand-written
    // branchless `(if t b)` is the same shape and the same argument.
    if !value::symbol_is(head, kw::IF) || !(items.len() == 3 || items.len() == 4) {
        return None;
    }
    let (test, then) = (items[1], items[2]);
    let diverges = |form: Value| {
        crate::types::check::infer::expr_ty(heap, form, ctx).is_some_and(|t| t.is_never())
    };
    let (then_ctx, else_ctx) = crate::types::check::guards::branch_scopes(heap, test, ctx);
    // The THEN arm cannot have run, so the sequel is the else-scope — the `when` case.
    if diverges(then) {
        return Some(else_ctx);
    }
    // …and symmetrically for `unless`, where the diverging arm is the else.
    if items.len() == 4 && diverges(items[3]) {
        return Some(then_ctx);
    }
    None
}

/// Walk a single-clause `fn` literal with each parameter bound to an INFERRED bound —
/// a plain binding (`dynamic_within`, never a sig-authoritative type), because an
/// inferred accumulator is an over-approximation: reading it by inclusion would flag
/// `(- x best)` under `(if best …)` as "`(not nil)` is not a number". Positions past
/// `tys` bind unknown. Body forms are checked in that scope; nothing else `check_fn_seeded`
/// does (the declared-return check, the dead-clause eligibility) applies to an inferred seed.
pub(super) fn check_fn_bound(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    tys: &[Ty],
) {
    let tys: Vec<Option<Ty>> = tys.iter().cloned().map(Some).collect();
    check_fn_bound_opt(heap, items, ctx, out, &tys)
}

/// [`check_fn_bound`] with a position the seed could not type left UNKNOWN (`None`) —
/// bound like an unseeded parameter — rather than `any`, which reads as a known top type
/// to the relations that ask.
pub(super) fn check_fn_bound_opt(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    tys: &[Option<Ty>],
) {
    check_fn_bound_with(heap, items, ctx, out, tys, false)
}

/// [`check_fn_bound_opt`], with `derived` marking the seed as CALLER-DERIVED (the
/// `let`-bound literal's, `derived_let_lambda`): each parameter is then bound through
/// `Ctx::bind_derived`, so the impossible-predicate lint leaves its guards alone — a
/// defensive `(int? b)` the in-scope callers never exercise is not "never true", it is
/// there for the callers that are not here yet (the private-`defn` rule, ADR-341).
pub(super) fn check_fn_bound_with(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    tys: &[Option<Ty>],
    derived: bool,
) {
    // A CLAUSE-style callback — `(fn ((acc n) …) (((x y & r) "+") …))` — gets the seed too,
    // per clause and through `bind_head`, so a destructuring head's binders are typed from
    // the position they destructure. Without this the whole seed was dropped for anything
    // multi-clause and every binder read `any`, which is why `(+ x y)` over a list of
    // strings went unreported in exactly the shape that motivated the seeding.
    //
    // Each clause is walked in its OWN scope: unlike the unseeded path, which unions every
    // clause's params to keep them from looking unbound, a type belongs to one clause and
    // carrying it into a sibling's body would be a fresh way to be wrong.
    let clause_forms = {
        let forms = &items[1..];
        let forms = match forms.first() {
            Some(Value::Str(_)) if forms.len() > 1 => &forms[1..],
            _ => forms,
        };
        // CLAUSE form, one clause or many: every form is `(head-list body…)`. Keyed on the
        // shape rather than on `fn_is_arity_multi_clause`, which is false for a single
        // clause — and a single-clause `(fn ((p) …))` then fell to the param-list path
        // below, where `(p)` is not the parameter list but the whole clause.
        (!forms.is_empty()
            && forms
                .iter()
                .all(|&c| crate::eval::macros::is_clause(heap, c)))
        .then_some(forms)
    };
    if let Some(forms) = clause_forms {
        for &clause in forms {
            let Some(citems) = list_items(heap, clause) else {
                continue;
            };
            let Some(&plist) = citems.first() else {
                continue;
            };
            let mut scope = ctx.clone();
            let heads = list_items(heap, plist).unwrap_or_default();
            for (i, &h) in heads.iter().enumerate() {
                scope = crate::types::check::sigs::bind_head(
                    heap,
                    scope,
                    h,
                    tys.get(i).cloned().flatten(),
                );
            }
            for &body_form in &citems[1..] {
                check_into(heap, body_form, &scope, out);
            }
        }
        return;
    }
    let Some(&params_form) = items.get(1) else {
        return;
    };
    let mut scope = ctx.clone();
    for (i, p) in fn_params(heap, params_form).into_iter().enumerate() {
        let ty = tys.get(i).cloned().flatten();
        scope = if derived {
            scope.bind_derived(p, ty)
        } else {
            scope.bind(p, ty)
        };
    }
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    lint_discarded_symbols(heap, &items[body_start..], items[0], &scope, out);
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// `(fn (params...) docstring? body...)` (and `lambda` — the same closure
/// shape) — parse the parameter list, bind each into `ctx`, then walk the body
/// in the extended scope. Parameter positions (`& rest`, `&optional`) are
/// binders, not references, so they're not flagged as unbound.
pub(super) fn check_fn(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    check_fn_seeded(heap, items, ctx, out, None, None);
}

/// What a declared `(sig …)` binds each parameter of a single-arm `fn` to — one entry per
/// parameter, `(type, sig-authoritative)` — and the sig itself when its shape fits the
/// parameter list (else `None`, and every entry is unknown). The ONE rule for seeding a
/// body from its declaration, shared by the walk (`check_fn_seeded`) and the caller-derived
/// site collector (`sigs::collect_private_sites`): the collector used to bind an
/// `&optional` function's parameters to nothing, so a site inside one handed every
/// callee unknowns where the walk saw the declared ints.
///
/// The closure's actual param count must fall inside the declared sig's arity range for
/// seeding to make sense: at least `params.len()` required, at most `params.len() +
/// optional.len()` unless it has a rest tail (any count at or above `params.len()` is then
/// fine). …or the declaration names exactly the REQUIRED positions and says nothing about
/// the optionals — `(sig f (int int -> int))` over `(defn f (a b &optional (c nil)) …)`.
/// The required positions align exactly, so those seed; each undeclared optional is bound
/// as an unseeded optional (its default's type, or unknown). Refusing the whole declaration
/// left every parameter of such a function unknown — bedit's `ed-visible-lines`, ten
/// declared parameters over ten required and two optionals, read `(+ y k)` as `number`
/// with `y` declared `int` two lines up.
///
/// An `&optional` position may genuinely be absent at the call site: an unsupplied optional
/// WITH a default `(n 1)` is bound to the default, never to nil — so the absence case is the
/// default's type (seeding it as `T | nil` made `(+ p n)` under `&optional (n 1)` read
/// `nil | int` and every such site a strict warning; a default whose type can't be pinned
/// keeps the nil reading, a superset). It is a plain (not sig-authoritative) binding, so a
/// defensive `(nil? p)` in the body is never mistaken for dead code the way an exact
/// required-param contract would be. The `& rest` binder (always last) collects the
/// variadic arguments into a list, so its type is `nil | list<rest-elem>` — not the element
/// type the sig's rest position carries (seeding it as the element was a false-positive
/// source: `(defn f (& xs) (reduce xs 0 +))` under `(sig f (& int -> …))` flagged `reduce`
/// for an int where a sequence is wanted), and `nil` beside it because a call that supplies
/// no rest argument binds the collector to `nil`. Plain too, so no dead-clause lint keys
/// off it.
pub(in crate::types::check) fn seeded_param_types<'s>(
    heap: &Heap,
    params_form: Value,
    sig: Option<&'s crate::types::Sig>,
    ctx: &Ctx,
) -> (Vec<(Option<Ty>, bool)>, Option<&'s crate::types::Sig>) {
    let params = fn_params(heap, params_form);
    let defaults = fn_param_defaults(heap, params_form);
    let has_rest = params_form_has_rest(heap, params_form);
    let required = required_param_count(heap, params_form);
    let sig = sig.filter(|s| {
        (params.len() >= s.params.len()
            && (s.rest.is_some() || params.len() <= s.params.len() + s.optional.len()))
            || (!has_rest
                && s.rest.is_none()
                && s.optional.is_empty()
                && required == s.params.len())
    });
    let seeded = (0..params.len())
        .map(|i| {
            if has_rest && i + 1 == params.len() {
                let rest_ty = sig
                    .and_then(|s| s.rest.clone())
                    .map(|elem| Ty::list_of(elem).union(Ty::of(crate::core::value::Tag::Nil)));
                return (rest_ty, false);
            }
            let is_optional_pos =
                sig.is_some_and(|s| i >= s.params.len() && i < s.params.len() + s.optional.len());
            match sig.and_then(|s| s.param(i)) {
                Some(ty) if is_optional_pos => {
                    let absent = defaults
                        .get(i)
                        .copied()
                        .flatten()
                        .and_then(|d| expr_ty(heap, d, ctx))
                        .unwrap_or(Ty::of(crate::core::value::Tag::Nil));
                    (Some(ty.union(absent)), false)
                }
                Some(ty) => (Some(ty), true),
                None => (None, false),
            }
        })
        .collect();
    (seeded, sig)
}

/// `check_fn`, optionally seeding the parameters from a `(sig …)` signature — used
/// when this `fn` is the value of a `(def name …)` whose `name` is declared. Each
/// parameter is then bound to its declared type *and* marked a sig-typed param,
/// so the body's checks know the types and a guard narrowing a parameter to the
/// empty type surfaces as a dead clause (`check_if`). Seeds only on an exact
/// positional match (no rest, equal arity) so positions can't misalign.
pub(super) fn check_fn_seeded(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    sig: Option<&crate::types::Sig>,
    name: Option<Symbol>,
) {
    // Multi-arity `fn` — `(fn ((a) …) ((a b) …))` — isn't one param list + body;
    // each form (after an optional docstring) is a clause `(param-list body…)`.
    // Bind *every* clause's params into one scope and walk every body. Over-binding
    // (a param from clause N visible in clause M's body) only widens scope, so it
    // can never manufacture a false positive — it just stops a param used in one
    // clause from looking unbound. The sig seeding (single positional match) doesn't
    // apply to a multi-arity callee, so it's dropped here.
    if crate::eval::macros::fn_is_arity_multi_clause(heap, items) {
        let forms = &items[1..];
        let forms = match forms.first() {
            Some(Value::Str(_)) if forms.len() > 1 => &forms[1..],
            _ => forms,
        };
        let mut scope = ctx.clone();
        for &clause in forms {
            if let Some(citems) = list_items(heap, clause) {
                if let Some(&plist) = citems.first() {
                    for p in fn_params(heap, plist) {
                        scope = scope.bind(p, None);
                    }
                }
            }
        }
        for &clause in forms {
            if let Some(citems) = list_items(heap, clause) {
                let body = citems.get(1..).unwrap_or(&[]);
                lint_discarded_symbols(heap, body, clause, &scope, out);
                for &body_form in body {
                    check_into(heap, body_form, &scope, out);
                }
            }
        }
        return;
    }
    let Some(&params_form) = items.get(1) else {
        return;
    };
    let params = fn_params(heap, params_form);
    let (seeded, sig) = seeded_param_types(heap, params_form, sig, ctx);
    let mut scope = ctx.clone();
    for (&p, (ty, authoritative)) in params.iter().zip(seeded) {
        scope = match (ty, authoritative) {
            (Some(ty), true) => scope.bind_sig_param(p, ty),
            (ty, _) => scope.bind(p, ty),
        };
    }
    // Skip a leading docstring (a lone string when more body follows).
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
    // Return-type check (a `GradualTy` consumer): the body's last form is the
    // function's result, which must be *consistent* with the declared return `R`.
    // `gradual_of` makes an over-approximated result (a call) `dynamic`, so the ∩
    // relation only warns on a body type provably disjoint from `R` — never on a
    // widened guess (a `number`-returning body declared `int` defers). A precise
    // literal return uses `⊆`. Only fires with a seeded (sig-matched) sig.
    if let Some(s) = sig {
        if let Some(&ret_form) = items[body_start..].last() {
            let g = gradual_of(heap, ret_form, &scope);
            // A body that yields `never` **never returns** — it always throws — so it is
            // consistent with every declared return, including `never` itself. Without
            // this skip the dynamic half of `consistent_with` (`∩ ≠ ⊥`) reads `never` as
            // disjoint from everything, since it is: an empty set shares no value with
            // any set, itself included. The result was a false positive on every
            // always-throwing function that carried a `(sig …)` — `declared return type
            // string but the body yields never` — and, at its silliest, on a function
            // declared `never` whose body yields `never`. It stayed hidden only because
            // so few signatures were declared; adopting them across `std/` surfaced it.
            // The argument check has carried the same skip, for the same reason, all
            // along.
            if !g.bound.is_never()
                && !g.consistent_with_mode(s.ret.clone(), ctx.strict())
                && !ctx.is_suppressed(crate::types::check::ctx::SUPPRESS_TYPE_MISMATCH)
            {
                let who = name
                    .map(|n| format!("{}: ", name_of(n)))
                    .unwrap_or_default();
                out.push((
                    heap.form_pos_only(ret_form),
                    format!(
                        "{}declared return type {} but the body yields {}",
                        who,
                        crate::types::check::annot::display_ty(&s.ret),
                        crate::types::check::annot::display_ty(&g.bound)
                    ),
                ));
            }
        }
    }
}

/// `(def name value)` — the binder is in position 1, the value in 2. Don't
/// flag `name` as an unbound *reference* (it's a binder); walk `value` as an
/// expression. `name` is added to the file-globals accumulator inside
/// [`check_file`], not here (which checks one form in isolation).
pub(super) fn check_def(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(&value_form) = items.get(2) else {
        // `(def name)` — degenerate; skip.
        return;
    };
    // `(def name (fn …))` where `name` carries a `(sig …)` — the shape `defn`
    // expands to. Seed the fn's params with the declared types so the body knows
    // them (and a guard narrowing a param to `never` becomes a dead clause).
    if let Some(&Value::Sym(name)) = items.get(1) {
        // A `:total` declaration (ADR-351) is checked BY the walk: every `match` failure
        // the body can reach has to be proven covered (`walk.rs`'s throw site), so the
        // body's scope carries the name it is held to.
        let total_scope;
        let ctx = if super::super::properties::has_prop(heap, ctx, name, "total") {
            total_scope = ctx.with_total_fn(name);
            &total_scope
        } else {
            ctx
        };
        // `ctx.declared_sig` is keyed by the *bare* name Pass 2.5 recorded from
        // the file's un-expanded `(sig …)` text; `name` here is `defn`'s
        // *expanded* def head, which is module-qualified inside a `defmodule`
        // block. The two only match at the root namespace — so a
        // `defmodule`-wrapped `(sig f …)` + `(defn f …)` pair needs the same
        // heap-wide fallback `gradual_of`'s reference branch already has
        // (ADR-124): `declared_heap_sig` reads the qualified store
        // `%register-sig` populates, so it matches regardless of namespace.
        let declared = ctx
            .declared_sig(name)
            .or_else(|| declared_heap_sig(heap, name));
        if let Some(sig) = declared {
            if let Some(fn_items) = fn_form_items(heap, value_form) {
                check_fn_seeded(heap, &fn_items, ctx, out, Some(&sig), Some(name));
                return;
            }
        }
        // A module-PRIVATE function with no declaration: its parameters are what its
        // callers in this file hand it (Pass 2.9, ADR-341) — bound plainly, not as a
        // sig-authoritative contract, so a defensive guard the callers never exercise
        // is not reported as a dead clause.
        if let Some(derived) = ctx.derived_params(name).cloned() {
            if let Some(fn_items) = fn_form_items(heap, value_form) {
                let params = fn_params(heap, fn_items[1]);
                if params.len() == derived.len()
                    && !crate::eval::macros::fn_is_arity_multi_clause(heap, &fn_items)
                {
                    let mut scope = ctx.clone();
                    for (p, ty) in params.iter().zip(derived) {
                        scope = scope.bind_derived(*p, ty);
                    }
                    scope = scope.with_derived_count_aliases(name, &params);
                    let body_start = match (fn_items.get(2), fn_items.get(3)) {
                        (Some(Value::Str(_)), Some(_)) => 3,
                        _ => 2,
                    };
                    for &body_form in &fn_items[body_start..] {
                        check_into(heap, body_form, &scope, out);
                    }
                    return;
                }
            }
        }
        // Gradual-assignment check (the first `GradualTy` consumer): when `name`
        // carries a non-arrow `(sig name T)`, the assigned value must be
        // *consistent* with `T`. A dynamic value (a redefinable global, an
        // unknown) defers; a value whose type is provably incompatible with `T`
        // is flagged. Sound: `consistent_with` only rejects a provable mismatch
        // (`bound ∩ T = ⊥`, or a precise literal `⊄ T`), never a widened guess.
        let declared_value_ty = ctx
            .declared_value_ty(name)
            .or_else(|| declared_heap_value_ty(heap, name));
        if let Some(t) = declared_value_ty {
            let g = gradual_of(heap, value_form, ctx);
            if !g.consistent_with_mode(t.clone(), ctx.strict()) {
                out.push((
                    heap.form_pos_only(form),
                    format!(
                        "{}: value of type {} is not assignable to declared type {}",
                        name_of(name),
                        g.bound,
                        t,
                    ),
                ));
            }
        }
    }
    // The value slot is evaluated — a bare unbound symbol there (`(def x typo)`)
    // is a reference error, same rule as a call operand.
    check_value_leaf(heap, value_form, form, ctx, out);
    check_into(heap, value_form, ctx, out);
}

/// `(defn name (params...) docstring? body...)` and the structurally identical
/// `defmacro` — the body lives in a fresh scope with `params` bound. Like
/// `def`, the `name` is a binder, not a reference; file-global accumulation
/// happens in [`check_file`].
pub(super) fn check_defn(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(&params_form) = items.get(2) else {
        return;
    };
    // Un-expanded `defn` path (e.g. `(check 'form)` without expansion). Whole-file
    // checking expands `defn` to `(def name (fn …))` first, so a sig'd function's
    // params are actually seeded in `check_def`; here there's no declared sig to
    // consult, so just bind the params.
    let mut scope = ctx.clone();
    for p in fn_params(heap, params_form) {
        scope = scope.bind(p, None);
    }
    let body_start = match (items.get(3), items.get(4)) {
        (Some(Value::Str(_)), Some(_)) => 4,
        _ => 3,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// `(if test then else?)` — check the test in the outer ctx, then descend
/// into each branch with the ctx narrowed by what the test would assert.
/// `else` defaults to `nil` (matches the evaluator), so absent or non-pair
/// branches simply contribute no warnings.
pub(super) fn check_if(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let test = items.get(1).copied().unwrap_or(Value::nil());
    let then_form = items.get(2).copied().unwrap_or(Value::nil());
    let else_form = items.get(3).copied().unwrap_or(Value::nil());

    // All three slots are evaluated value positions — a bare unbound symbol in
    // any (`(if typo …)`) is a reference error. then/else use the narrowed ctx,
    // matching how they're walked.
    check_value_leaf(heap, test, form, ctx, out);
    check_into(heap, test, ctx, out);

    // **Match-redundancy lint** (ADR-122). If this `if`'s test is itself a
    // literal `%eq` guard, scan forward through the `else`-chain for another
    // test of the same symbol against the same literal — the shape
    // `match`/`cond` compile duplicate clauses into (whichever occurs first
    // always wins, so a later one is dead code). Purely structural — no
    // scrutinee `Ty` involved, so this fires on any hand-written same-symbol
    // `%eq`-if chain too, not just `match`-generated ones.
    if !ctx.is_suppressed(crate::types::check::ctx::SUPPRESS_UNREACHABLE) {
        if let Some((sym, lit)) = literal_eq_test_raw(heap, test) {
            if let Some(dup) = find_redundant_clause(heap, else_form, sym, lit) {
                let label =
                    render_literal_pattern(heap, lit).unwrap_or_else(|| "this value".to_string());
                out.push((
                    heap.form_pos_only(dup),
                    format!("match: unreachable clause — {label} is already handled above"),
                ));
            }
        }
    }

    // **Truthy-failure lint.** A `failure` is TRUTHY in Brood, so `(if (string/->number s) …)`
    // takes the THEN branch precisely when the parse failed — the opposite of what the shape
    // reads as. The checker already catches the downstream consequence when the value flows
    // into something typed (`(+ n 1)` → "expects number, got number | failure"), but an
    // untyped consumer, or a test whose only job IS the branch, sails through. This class has
    // a habit: a registry shipped `(string/->number id)` straight into a query and answered
    // 500 on any non-numeric URL.
    //
    // Positively-known failures only, the ADR-310 rule: a bound known merely by exclusion
    // (`any`, or a guard's `(not nil)`) admits failure the way it admits everything and says
    // nothing, and reading that as "can fail" would fire on every unannotated parameter.
    if !ctx.is_suppressed(crate::types::check::ctx::SUPPRESS_TYPE_MISMATCH) {
        if let Some(ty) = crate::types::check::infer::expr_ty(heap, test, ctx) {
            if ty.contains_tag(crate::types::Tag::Failure) && !ty.is_known_only_by_exclusion() {
                // The test is often a bare symbol or a macro-expanded form with no position
                // of its own; the enclosing `if` always has one, and a warning without a
                // line is a warning nobody can act on.
                let pos = heap
                    .form_pos_only(test)
                    .or_else(|| heap.form_pos_only(form));
                out.push((
                    pos,
                    "a failure value is TRUTHY, so this tests as true when the operation \
                     FAILED — narrow with the type you want (`int?`, `bytes?`, `string?`) \
                     or test `(failure? …)` explicitly"
                        .to_string(),
                ));
            }
        }
    }

    let (then_ctx, else_ctx) = match guard_assertion(heap, test, ctx) {
        Some(g) => {
            // An `else_only` guard (`(empty? xs)`) asserts nothing when true: the
            // then-branch keeps the scope, and the dead-clause lint has nothing to judge.
            let then_ctx = if g.else_only {
                ctx.clone()
            } else {
                ctx.narrow(g.sym, g.ty.clone())
            };
            // **Dead-clause lint.** If the guard narrowed a dead-clause-eligible
            // binding — a *sig-typed parameter* or a *precise surface `let`-local*
            // (ADR-131) — to the empty type, this branch can never run: the
            // binding's type is disjoint from what the guard (a `cond` predicate or
            // a `match` literal pattern, reached here via the scrutinee alias)
            // asserts. Eligibility (see `Ctx::dead_clause_locals`) is the whole of
            // the surface-vs-generated scoping: it admits only a non-gensym,
            // precisely-typed, immutable local, so a literal scrutinee, a
            // redefinable global, a call-result, and every macro-generated temp are
            // ruled out at the *binding*, never at the guard site — exactly how the
            // sig-param lint stays false-positive-free without inspecting positions.
            if let Some((p, known)) = then_ctx.newly_dead_binding(ctx) {
                out.push((
                    heap.form_pos_only(form),
                    format!(
                        "unreachable clause: {} is {}, which can never be {} \
                         — this branch is dead code",
                        name_of(p),
                        known,
                        g.ty,
                    ),
                ));
            }
            // Only narrow the else-branch when the guard is biconditional — a
            // `then_only` guard (the `and` short-circuit) doesn't establish `¬ty`
            // on a falsy test, so negating there would be a false positive.
            let else_ctx = if g.then_only {
                ctx.clone()
            } else {
                ctx.narrow(g.sym, g.else_type())
            };
            (then_ctx, else_ctx)
        }
        None => (ctx.clone(), ctx.clone()),
    };
    // Layer a **path** narrowing on top (occurrence typing through a `(get base
    // :key)` access): `(if (int? (get r :age)) …)` types `(get r :age)` as `int`
    // in the then-branch (and `¬int` in the else, for a biconditional predicate).
    // The base must not be a GLOBAL — see the note in `guards::branch_scopes`:
    // immutability makes a local stable between the guard and the use, and says nothing
    // about a global, which another process can `def` in between.
    let (then_ctx, else_ctx) = match path_guard_assertion(heap, test)
        .filter(|pg| !crate::types::check::sigs::is_globally_bound(heap, pg.base))
    {
        Some(pg) => {
            // Precise field-access narrowing for the exact path (both branches).
            let t = then_ctx.narrow_path(pg.base, pg.keys.clone(), pg.ty.clone());
            // Refine the *base*'s record type in the then-branch so the narrowing
            // flows when `base` is passed to a function (or otherwise used as a
            // value). Sound only in the then-branch: the guard being true proves
            // the whole access chain is present and typed. Only when every step is
            // a *field* — `base` is then an open record `{k1: {… {kn: ty}}}` (built
            // inner-out). A path with an *index* step would need a fixed-arity
            // tuple/vector refinement we can't infer from one position, so base
            // refinement is skipped there (the path narrowing above still applies).
            let all_fields: Option<Vec<_>> = pg
                .keys
                .iter()
                .map(|k| match k {
                    PathKey::Field(s) => Some(*s),
                    PathKey::Index(_) | PathKey::Call(_) => None,
                })
                .collect();
            let t = match all_fields {
                Some(fields) => {
                    // **Open** (ADR-264): the guard proves this path is present and
                    // typed, and says nothing about the base's other keys — a closed
                    // shape here would claim they are absent, which the guard never
                    // established.
                    let base_record = fields.iter().rev().fold(pg.ty.clone(), |acc, &k| {
                        Ty::record_of_open(std::iter::once((k, (acc, true))).collect())
                    });
                    t.narrow(pg.base, base_record)
                }
                None => t,
            };
            let e = if pg.then_only {
                else_ctx
            } else {
                else_ctx.narrow_path(pg.base, pg.keys, pg.ty.negate())
            };
            (t, e)
        }
        None => (then_ctx, else_ctx),
    };
    // Layer **chained-guard** narrowing on top: every conjunct of an `and`-test narrows
    // the then-branch (a truthy `and` proves all of them), every biconditional disjunct of
    // an `or`-test narrows the else-branch by its complement (a falsy `or` refutes all of
    // them, each on its own variable), and an `or`-test whose disjuncts are all
    // biconditional guards over ONE variable narrows the then-branch to their union. All via intersecting `narrow`, so they compose with
    // the single-guard and path narrowings above. Guards read against the original `ctx`
    // (for let-alias resolution); the tightening lands on the branch ctxs.
    let (then_ctx, else_ctx) = {
        let mut t = then_ctx;
        let mut e = else_ctx;
        for g in and_conjunct_guards(heap, test, ctx) {
            t = t.narrow(g.sym, g.ty);
        }
        for g in or_disjunct_guards(heap, test, ctx) {
            e = e.narrow(g.sym, g.else_type());
        }
        if let Some((sym, union)) = or_same_var_narrowing(heap, test, ctx) {
            t = t.narrow(sym, union);
        }
        // …and a comparison's intervals, lengths and index bounds (ADR-350).
        apply_comparison_facts(heap, test, ctx, t, e)
    };
    // A branch whose scope is contradicted by its own test cannot run: don't check it.
    if !then_ctx.is_dead() {
        check_value_leaf(heap, then_form, form, &then_ctx, out);
        check_into(heap, then_form, &then_ctx, out);
    }
    if !else_ctx.is_dead() {
        check_value_leaf(heap, else_form, form, &else_ctx, out);
        check_into(heap, else_form, &else_ctx, out);
    }
}

/// The scope after ONE `let` binding `(pat rhs)`, given the RHS's type as read in the
/// pre-bind scope — the one place the rules live, so every walker that must see a
/// `let` body the way `check_let` does (the caller-derived site collector,
/// `sigs::collect_private_sites`) binds identically. A symbol binder gets the type, a
/// `fn` literal's parameter domains as a per-name fact (`sigs::let_bound_lambda_sig`),
/// a BICONDITIONAL guard result as a guard alias (a `then_only`/`else_only` one must
/// not be, or a later `(if alias …)` would negate it in the other branch — the `and`
/// short-circuit), and a plain `(let (name other) …)` as an alias of `other` — the
/// `match` compiler's `(let (m__28 x) (if (%eq m__28 lit) …))` narrows the user's `x`
/// through it, and `and`/`or`'s temporaries narrow the value they stand for. `other` is
/// not required to be a known local: narrowing inside a branch is sound on a free
/// reference too (vacuously, on an unreachable path). A destructuring binder binds each
/// symbol leaf to its position's type.
pub(in crate::types::check) fn let_bind_scope(
    heap: &Heap,
    scope: Ctx,
    pat: Value,
    rhs: Value,
    rhs_ty: Option<Ty>,
) -> Ctx {
    let Value::Sym(name) = pat else {
        let mut scope = scope;
        for (sym, ty) in pattern_bindings(heap, pat, rhs_ty.as_ref()) {
            scope = scope.bind(sym, ty);
        }
        return scope;
    };
    let rhs_guard = guard_assertion(heap, rhs, &scope);
    let mut scope = scope.bind(name, rhs_ty.clone());
    if let Some(sig) = super::super::sigs::let_bound_lambda_sig(heap, rhs, rhs_ty.as_ref(), &scope)
    {
        scope = scope.bind_let_fn_sig(name, sig);
    }
    if let Some(g) = rhs_guard {
        if !g.then_only && !g.else_only {
            scope = scope.add_guard(name, g.sym, g.ty, g.else_ty, false);
        }
    }
    // A `when`-shaped binding — `(let (src (when k (lookup k))) …)`, which is `(if k E nil)`
    // once expanded — is a guard on `k`: a truthy `src` proves `k` truthy (a falsy `k` makes
    // the value `nil`), so `(cond src (use k) …)` reads `k` narrowed in that branch. A falsy
    // `src` proves nothing (`E` may be nil), hence then-only. Only for a lexical local `k`.
    if let Some(items) = list_items(heap, rhs) {
        let when_shaped = items.len() == 3 || (items.len() == 4 && matches!(items[3], Value::Nil));
        if let (Some(Value::Sym(head)), Some(Value::Sym(k))) = (items.first(), items.get(1)) {
            if when_shaped && value::symbol_is(*head, kw::IF) && scope.is_lexical_local(*k) {
                scope = scope.add_guard(name, *k, Ty::truthy(), None, true);
            }
        }
    }
    if let Value::Sym(target) = rhs {
        scope = scope.add_alias(name, target);
    }
    // `(let (n (count xs)) …)`: `n` is the length of `xs` for the scope (ADR-350), so a
    // guard on `n` narrows `xs`'s length and bounds an index of it.
    if let Some(items) = list_items(heap, rhs) {
        if let [Value::Sym(head), Value::Sym(target)] = items[..] {
            let counts = value::symbol_is(head, "count")
                || value::symbol_is(head, "string/length")
                || value::symbol_is(head, "vector-length");
            if counts && !scope.is_lexical_local(head) && scope.is_lexical_local(target) {
                scope = scope.add_count_alias(name, target);
            }
        }
    }
    scope
}

/// `(let bindings body…)` / `(letrec …)` — walk the bindings,
/// then check the body in the extended ctx. `letrec` pre-binds every name to
/// "in scope, type unknown" before walking RHSs, matching the evaluator's
/// nil-pre-bind so a self/mutual-recursive call inside a RHS isn't flagged
/// unbound. `let`/`let*` walk sequentially — each RHS sees only the
/// previously-bound names. (The let-vs-let* scope distinction doesn't affect
/// the unbound check since we only widen names; type-flow stays sound.)
///
/// Quietly skips a malformed bindings shape (a pattern-target `let`, an
/// improper list, an odd number of binding items): those are evaluator-level
/// errors and aren't this checker's job.
pub(super) fn check_let(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    letrec: bool,
) {
    let Some(&binds_form) = items.get(1) else {
        return;
    };
    let Some(binds) = bindings(heap, binds_form) else {
        // Unknown shape — just recurse generically so we still check nested calls.
        for &item in items {
            check_into(heap, item, ctx, out);
        }
        return;
    };
    if binds.len() % 2 != 0 {
        return;
    }
    let mut scope = ctx.clone();
    // letrec: pre-bind every name to `None` (in scope, no known type) so each
    // RHS can refer to its peers (and to itself).
    if letrec {
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                scope = scope.bind(name, None);
            }
            j += 2;
        }
    } else {
        // Plain `let` is sequential, but a binding whose RHS is a `fn`/`lambda`
        // captures the let frame — the closure resolves its own binding name (and
        // its fn-valued siblings) by late lookup when *called*, so a self- or
        // mutually-recursive `let`-bound closure works at runtime. Pre-bind those
        // names so the unbound check agrees. Only fn-valued names, and only widening
        // scope, so an eager forward reference in a non-closure RHS still surfaces.
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                if fn_form_items(heap, binds[j + 1]).is_some() {
                    scope = scope.bind(name, None);
                }
            }
            j += 2;
        }
    }
    // A `fn`-valued binder's parameters are derived from its callers — the later bindings
    // and the body, which is everything the name is visible in (`let_lambda_derived_params`).
    // Those sites are typed in the scope every binder is bound in, so the scope is built
    // once ahead of the checking pass; only a let that binds a literal pays for it.
    let has_fn_binder = (0..binds.len())
        .step_by(2)
        .any(|j| matches!(binds[j], Value::Sym(_)) && fn_form_items(heap, binds[j + 1]).is_some());
    let derived_for: Vec<Option<(Vec<Option<Ty>>, Option<Ty>)>> = if has_fn_binder {
        let mut full = scope.clone();
        let mut j = 0;
        while j < binds.len() {
            let rhs_ty = expr_ty(heap, binds[j + 1], &full);
            full = let_bind_scope(heap, full, binds[j], binds[j + 1], rhs_ty);
            j += 2;
        }
        (0..binds.len())
            .step_by(2)
            .map(|j| {
                let Value::Sym(name) = binds[j] else {
                    return None;
                };
                let rhs = binds[j + 1];
                fn_form_items(heap, rhs)?;
                let visible: Vec<Value> = binds[j + 2..]
                    .iter()
                    .skip(1)
                    .step_by(2)
                    .copied()
                    .chain(items[2..].iter().copied())
                    .collect();
                derived_let_lambda(heap, name, rhs, &visible, &full)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut i = 0;
    while i < binds.len() {
        let (pat, rhs) = (binds[i], binds[i + 1]);
        // The RHS is an evaluated value position — a bare unbound symbol there
        // (`(let (x typo) …)`) is a reference error.
        check_value_leaf(heap, rhs, form, &scope, out);
        let derived = derived_for.get(i / 2).and_then(|d| d.as_ref());
        match (derived, fn_form_items(heap, rhs)) {
            (Some((tys, _)), Some(fn_items)) => {
                check_fn_bound_with(heap, &fn_items, &scope, out, tys, true)
            }
            _ => check_into(heap, rhs, &scope, out),
        }
        let rhs_ty = derived
            .and_then(|(tys, ret)| derived_let_lambda_arrow(tys, ret))
            .or_else(|| expr_ty(heap, rhs, &scope));
        // Is the RHS *precise* (non-redefinable)? Computed in the pre-bind scope.
        // `dynamic == false` ⇔ a literal / integer-closed expression, never a
        // call-result or global reference — the reload-safe subset the dead-clause
        // lint may key off (ADR-131).
        let rhs_precise = !gradual_of(heap, rhs, &scope).dynamic;
        scope = let_bind_scope(heap, scope, pat, rhs, rhs_ty.clone());
        // Dead-clause lint eligibility: a surface (non-gensym), precisely-typed
        // `let`-local joins the set the dead-clause lint may flag, so a later guard
        // that narrows it to `never` is caught — `(let (x 5) (cond (string? x) …))`.
        if let Value::Sym(name) = pat {
            if rhs_precise
                && rhs_ty.as_ref().is_some_and(|t| !t.is_never())
                && !is_gensym_sym(name)
                && heap.form_pos_only(form).is_some()
                && !heap.is_synthetic(form)
            {
                scope.mark_dead_clause_local(name);
            }
        }
        i += 2;
    }
    // The body is a SEQUENCE, so a guard that diverges narrows every form after it
    // (`diverging_guard_scope`) — the `(when (nil? root) (error …))` early-return idiom.
    let mut scope = scope;
    for &body_form in &items[2..] {
        check_into(heap, body_form, &scope, out);
        if let Some(next) = diverging_guard_scope(heap, body_form, &scope) {
            scope = next;
        }
    }
    // Unused let binding lint. For each bound name, warn if it never appears
    // as a Value::Sym in any part of its visible scope: subsequent binding
    // elements + the body (plus preceding binding elements for letrec, where
    // any RHS may reference any other name). The scan is conservative — it
    // counts occurrences in binder positions and in quoted forms, so the only
    // errors are false negatives (missed warnings), never false positives.
    //
    // Exempt: names starting with `_` (the "intentionally unused" convention).
    {
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                let nm = name_of(name);
                // Exempt gensym temporaries (`<prefix>__<n>`, value::gensym): a
                // macro expansion (match / pattern lowering) can attach its
                // call-site position to the generated `let`, so the position
                // check below doesn't catch them — but the name does.
                let is_gensym = is_gensym_sym(name);
                // Exempt a binding that *shadows* an existing global or curated
                // builtin (`(let (list …) …)`, `(let (= …) …)`): you never
                // accidentally name a local after a builtin, so a shadow left
                // unused is a deliberate scope-isolation / hygiene test, not a
                // leftover. (`_`-prefixing can't express it — that changes the
                // name being shadowed.)
                let shadows_global = is_globally_bound(heap, name)
                    || curated_sig(name).is_some()
                    || ctx.is_file_global(name);
                if !nm.starts_with('_') && !is_gensym && !shadows_global {
                    // letrec: also scan preceding elements (mutual recursion).
                    let preceding_used =
                        letrec && binds[..j].iter().any(|&f| sym_appears_in(heap, f, name));
                    let following_used = binds[j + 2..]
                        .iter()
                        .any(|&f| sym_appears_in(heap, f, name));
                    let body_used = items[2..].iter().any(|&f| sym_appears_in(heap, f, name));
                    if !preceding_used && !following_used && !body_used {
                        // Only warn for user-written `let`s. Compiler-generated lets (from
                        // match/pattern expansion) are exempt: their names are user-chosen
                        // but the "unused" status is an expansion artifact. They used to be
                        // told apart by having no source position; the expander now gives
                        // generated code the position of the form it came from, so the
                        // question is asked directly (`is_synthetic`).
                        if !heap.is_synthetic(form) {
                            if let Some(pos) = heap.form_pos_only(form) {
                                out.push((Some(pos), format!("unused let binding: {}", nm)));
                            }
                        }
                    }
                }
            }
            j += 2;
        }
    }
}

/// A `let`-bound `fn` literal's caller-derived parameter types and, under them, its result —
/// what both the walk (`check_let`, to check the body) and the inference (`expr_ty`'s `let`,
/// to type the name's calls) bind the name from. `visible` is every form the binding is
/// in scope for; `scope` the scope those forms are typed in. `None` when nothing could be
/// derived (no plain-parameter literal, no site, or the name escapes).
pub(in crate::types::check) fn derived_let_lambda(
    heap: &Heap,
    name: Symbol,
    rhs: Value,
    visible: &[Value],
    scope: &Ctx,
) -> Option<(Vec<Option<Ty>>, Option<Ty>)> {
    fn_form_items(heap, rhs)?;
    let derived = let_lambda_derived_params(heap, name, rhs, visible, scope)?;
    if derived.iter().all(Option::is_none) {
        return None;
    }
    // …and the literal's result under those inputs, for the arrow the name is bound to:
    // `(row-op 2)` then types as the body does over an int. A self-call inside the body
    // contributes ⊥ to that result — the least fixpoint: by induction a recursive call
    // returns something the non-recursive branches already cover, and a call with an
    // uninhabited argument is `never`, so `(inc (step next))` folds away too. Read under
    // the pre-bound (unknown) name instead, `(if … (step next) l)` was unknown, the arrow's
    // result unknown, and every caller of a recursive local helper read `any`.
    let self_arrow = Ty::arrow(Sig::new(vec![Ty::ANY; derived.len()], Ty::NEVER));
    let body_scope = scope.bind(name, Some(self_arrow));
    let ret = super::super::infer::callback_ret(heap, rhs, &derived, &body_scope);
    Some((derived, ret))
}

/// The arrow a derived `let`-bound literal is bound to (`infer::lambda_arrow`'s shape):
/// `any` parameters — an arrow's parameters are contravariant, and the derivation is a
/// fact about the callers, not a domain — and the result typed under what they hand over.
pub(in crate::types::check) fn derived_let_lambda_arrow(
    tys: &[Option<Ty>],
    ret: &Option<Ty>,
) -> Option<Ty> {
    ret.as_ref()
        .map(|r| Ty::arrow(Sig::new(vec![Ty::ANY; tys.len()], r.clone())))
}

/// The type a `let` binding's right-hand side is bound as, for the INFERENCE paths
/// (`infer`'s `let`, `gradual_of`'s): a `fn` literal bound to a name is typed from its
/// callers (`derived_let_lambda`, over the later bindings and the body), anything else by
/// `expr_ty`. `binds` is the whole binding list, `i` the binder's index in it, `items` the
/// `let` form's items, `scope` the scope so far. Shared so the three `let` typings agree
/// — the walk binds the same arrow in `check_let`.
pub(in crate::types::check) fn let_rhs_ty(
    heap: &Heap,
    binds: &[Value],
    i: usize,
    items: &[Value],
    scope: &Ctx,
) -> Option<Ty> {
    let derived_arrow = match binds[i] {
        Value::Sym(name) if fn_form_items(heap, binds[i + 1]).is_some() => {
            let visible: Vec<Value> = binds[i + 2..]
                .iter()
                .skip(1)
                .step_by(2)
                .copied()
                .chain(items[2..].iter().copied())
                .collect();
            // The name is in scope for its sites, bound to nothing yet — the pre-bind
            // `check_let` makes for a `fn`-valued binder. Without it a site's typing
            // resolves the name to a GLOBAL of the same spelling: `(reduce xs '() step)`
            // under `(let (step (fn (a v) a)) …)` read the file's `step` — `conj` — and
            // the local's derivation came back holding a list.
            let scope = scope.bind(name, None);
            derived_let_lambda(heap, name, binds[i + 1], &visible, &scope)
                .and_then(|(tys, ret)| derived_let_lambda_arrow(&tys, &ret))
        }
        _ => None,
    };
    derived_arrow.or_else(|| expr_ty(heap, binds[i + 1], scope))
}

/// The **caller-derived parameter types of a `let`-bound `fn` literal** — the same
/// derivation a module-private `defn` gets (`sigs::caller_derived_params`), scoped to the
/// binding: `name`'s callers are exactly the forms the binding is visible in (the later
/// bindings' right-hand sides and the body), so per parameter the answer is the union of
/// what every call there hands it, or what a combinator promises to call it with
/// (`walk::callback_seed` — `(map (range …) row-op)` hands an int). One position some
/// site cannot type is unknown.
///
/// Sound for the reason the private-`defn` derivation is: a call inside the let's scope
/// is the only way to reach the closure, unless the name ESCAPES — used as a value where
/// no callee promise types it (`(spawn f)`, inside a vector, quoted), rebound by a nested
/// binder, or reached through a macro the expander left as written — and every one of
/// those declines the whole derivation (`None`).
///
/// Before this the literal's parameters were unknown, so `(+ y k)` in
/// `(let (row-op (fn (k) … (+ y k))) (map (range 0 3) row-op))` read `number` and went
/// into an `int` parameter as a strict finding — fifteen of bedit's, every one a local
/// helper over an index — while the same literal written inline in the `map` was typed
/// from the element. Plain-symbol parameters only, single clause.
fn let_lambda_derived_params(
    heap: &Heap,
    name: Symbol,
    rhs: Value,
    visible: &[Value],
    scope: &Ctx,
) -> Option<Vec<Option<Ty>>> {
    let items = fn_form_items(heap, rhs)?;
    let params = list_items(heap, *items.get(1)?)?;
    if params
        .iter()
        .any(|p| !matches!(p, Value::Sym(s) if !name_of(*s).starts_with('&')))
    {
        return None;
    }
    let arity = params.len();
    let mut sites: Vec<Vec<Option<Ty>>> = Vec::new();
    for &form in visible {
        if !collect_let_lambda_sites(heap, name, arity, form, scope, &mut sites) {
            return None;
        }
    }
    if sites.is_empty() {
        return None;
    }
    let union_sites = |sites: &[Vec<Option<Ty>>]| -> Vec<Option<Ty>> {
        (0..arity)
            .map(|i| {
                let mut acc: Option<Ty> = None;
                for site in sites {
                    match site.get(i).cloned().flatten() {
                        Some(t) => acc = Some(acc.map_or(t.clone(), |a| a.union(t))),
                        None => return None,
                    }
                }
                acc
            })
            .collect()
    };
    let external = union_sites(&sites);
    // …and the literal's OWN self-calls, to a least fixpoint over its body: `(f (inc k))`
    // under `(f 0)` alone read `k` as the literal `0`, decided `(> k 3)` false, and typed
    // the result `never` — the recursive arm was the only live one. Each round binds the
    // parameters to what is derived so far, reads the self-call sites under them, unions
    // them with the external sites and widens an interval that moved (ADR-350), so `0`,
    // `0 | 1`, … is `int[0..]` in one step. A self-reference that is not a call (the name
    // escaping inside its own body) keeps the external derivation alone.
    let param_syms: Vec<Symbol> = params
        .iter()
        .filter_map(|p| match p {
            Value::Sym(s) => Some(*s),
            _ => None,
        })
        .collect();
    let body: Vec<Value> = items.iter().skip(2).copied().collect();
    let mut derived = external.clone();
    // Only a literal that names itself pays for the ascent: each round re-collects the
    // body, and a body's nested `let`s derive their own literals on the way, so an
    // unconditional loop multiplied the cost by its rounds at every level of nesting.
    if !body.iter().any(|&form| sym_appears_in(heap, form, name)) {
        return Some(derived);
    }
    for _ in 0..8 {
        let mut inner = scope.clone();
        for (p, t) in param_syms.iter().zip(&derived) {
            inner = inner.bind(*p, t.clone());
        }
        let mut self_sites: Vec<Vec<Option<Ty>>> = Vec::new();
        let sound = body.iter().all(|&form| {
            collect_let_lambda_sites(heap, name, arity, form, &inner, &mut self_sites)
        });
        if !sound || self_sites.is_empty() {
            break;
        }
        self_sites.extend(sites.iter().cloned());
        let mut next = union_sites(&self_sites);
        for (slot, prev) in next.iter_mut().zip(&derived) {
            if let (Some(n), Some(p)) = (slot.as_ref(), prev) {
                *slot = Some(n.widen_intervals_against(p));
            }
        }
        if next == derived {
            break;
        }
        derived = next;
    }
    Some(derived)
}

/// [`let_lambda_derived_params`]'s walk over one visible form: push a site per call of
/// `name` (its arguments typed in `scope`) and per handover a callee promises to call
/// (`callback_seed`); return `false` the moment `name` escapes.
fn collect_let_lambda_sites(
    heap: &Heap,
    name: Symbol,
    arity: usize,
    form: Value,
    scope: &Ctx,
    sites: &mut Vec<Vec<Option<Ty>>>,
) -> bool {
    match form {
        Value::Sym(s) => s != name,
        Value::Vector(id) => {
            let elems = heap.vector(id).to_vec();
            elems
                .into_iter()
                .all(|e| collect_let_lambda_sites(heap, name, arity, e, scope, sites))
        }
        Value::Map(id) => heap.map_entries(id).into_iter().all(|(k, v)| {
            collect_let_lambda_sites(heap, name, arity, k, scope, sites)
                && collect_let_lambda_sites(heap, name, arity, v, scope, sites)
        }),
        Value::Pair(_) => {
            let Some(items) = list_items(heap, form) else {
                return false;
            };
            let Some(&head) = items.first() else {
                return true;
            };
            if let Value::Sym(h) = head {
                // Quoted data: a mention there is data, not a reference — unless it IS the
                // name, which is then an escape the walk cannot follow.
                if value::symbol_is(h, kw::QUOTE) {
                    return !sym_appears_in(heap, form, name);
                }
                // A nested binder that rebinds `name` shadows it: the references beneath
                // are someone else's. Decline rather than sort them out.
                if is_fn_head(h) {
                    if items
                        .get(1)
                        .is_some_and(|&p| fn_params(heap, p).contains(&name))
                    {
                        return false;
                    }
                    // An unseeded literal (one no callee promised anything to): its
                    // parameters are unknown inside, and shadow whatever they are named.
                    let mut inner = scope.clone();
                    if let Some(&p) = items.get(1) {
                        for param in fn_params(heap, p) {
                            inner = inner.bind(param, None);
                        }
                    }
                    return items
                        .iter()
                        .skip(2)
                        .all(|&it| collect_let_lambda_sites(heap, name, arity, it, &inner, sites));
                }
                if value::symbol_is(h, kw::LET)
                    || value::symbol_is(h, kw::LETREC)
                    || value::symbol_is(h, "let*")
                {
                    let Some(binds) = items.get(1).and_then(|&b| bindings(heap, b)) else {
                        return false;
                    };
                    if binds.len() % 2 != 0
                        || binds
                            .iter()
                            .step_by(2)
                            .any(|&p| sym_appears_in(heap, p, name))
                    {
                        return false;
                    }
                    // Each binder in scope for what follows it, so a site under
                    // `(let ([line from to] (nth rows k)) (row-op k line from to))` types
                    // its arguments — the same sequential binding `expr_ty`'s `let` does.
                    let mut inner = scope.clone();
                    let mut j = 0;
                    while j < binds.len() {
                        if !collect_let_lambda_sites(heap, name, arity, binds[j + 1], &inner, sites)
                        {
                            return false;
                        }
                        let rhs_ty = let_rhs_ty(heap, &binds, j, &items, &inner);
                        match binds[j] {
                            Value::Sym(b) => inner = inner.bind(b, rhs_ty),
                            pat => {
                                for (sym, ty) in pattern_bindings(heap, pat, rhs_ty.as_ref()) {
                                    inner = inner.bind(sym, ty);
                                }
                            }
                        }
                        j += 2;
                    }
                    for &it in &items[2..] {
                        if !collect_let_lambda_sites(heap, name, arity, it, &inner, sites) {
                            return false;
                        }
                        if let Some(next) = diverging_guard_scope(heap, it, &inner) {
                            inner = next;
                        }
                    }
                    return true;
                }
                // The scope a site sits in must be what the WALK sees there (the rule
                // `sigs::collect_private_sites` learned): a `match` arm's `(ok? m)` under
                // the arm's own length test hands `m` a `string`, not the `nil | string`
                // `first` answers unguarded. Same guard rule, same diverging-guard
                // sequencing over a body.
                if value::symbol_is(h, kw::IF) && (items.len() == 3 || items.len() == 4) {
                    if !collect_let_lambda_sites(heap, name, arity, items[1], scope, sites) {
                        return false;
                    }
                    let (then_scope, else_scope) =
                        crate::types::check::guards::branch_scopes(heap, items[1], scope);
                    if !collect_let_lambda_sites(heap, name, arity, items[2], &then_scope, sites) {
                        return false;
                    }
                    return items.get(3).is_none_or(|&else_form| {
                        collect_let_lambda_sites(heap, name, arity, else_form, &else_scope, sites)
                    });
                }
                if value::symbol_is(h, kw::DO) {
                    let mut seq = scope.clone();
                    for &it in &items[1..] {
                        if !collect_let_lambda_sites(heap, name, arity, it, &seq, sites) {
                            return false;
                        }
                        if let Some(next) = diverging_guard_scope(heap, it, &seq) {
                            seq = next;
                        }
                    }
                    return true;
                }
                // Syntax the expander left as written may construct a call the walk
                // cannot see.
                if super::resolves_to_macro(heap, scope, h) {
                    return false;
                }
                // A direct call: a site when the arity fits (a wrong arity raises and
                // contributes nothing); the arguments are walked either way.
                if h == name {
                    if items.len() - 1 == arity {
                        sites.push(
                            items[1..]
                                .iter()
                                .map(|&a| expr_ty(heap, a, scope))
                                .collect(),
                        );
                    }
                    return items
                        .iter()
                        .skip(1)
                        .all(|&a| collect_let_lambda_sites(heap, name, arity, a, scope, sites));
                }
            }
            // A handover: `name` where the callee promises what it will call it with.
            let handed = super::calls::callback_seed(
                heap,
                form,
                &items,
                scope,
                &|arg, wanted| matches!(arg, Value::Sym(s) if s == name && wanted == arity),
            );
            // …and a callback LITERAL at a promised position is walked with its parameters
            // seeded, so a site inside `(mapcat xs (fn (line) (row-op (- line top) …)))`
            // types `line` from the element — exactly as the walk checks that literal.
            let seeded_literal = super::calls::callback_seed(
                heap,
                form,
                &items,
                scope,
                &super::calls::literal_fits(heap),
            );
            for (i, &it) in items.iter().enumerate() {
                if let Some((idx, sig)) = &handed {
                    if *idx == i {
                        sites.push(sig.params.iter().cloned().map(Some).collect());
                        continue;
                    }
                }
                if i == 0 && matches!(it, Value::Sym(_)) {
                    continue;
                }
                if let Some((idx, sig)) = &seeded_literal {
                    if *idx == i {
                        let Some(lit) = fn_form_items(heap, it) else {
                            return false;
                        };
                        let Some(&plist) = lit.get(1) else {
                            return false;
                        };
                        let params = fn_params(heap, plist);
                        if params.contains(&name) {
                            return false;
                        }
                        let mut inner = scope.clone();
                        for (k, param) in params.into_iter().enumerate() {
                            inner = inner.bind(param, sig.params.get(k).cloned());
                        }
                        if !lit
                            .iter()
                            .skip(2)
                            .all(|&b| collect_let_lambda_sites(heap, name, arity, b, &inner, sites))
                        {
                            return false;
                        }
                        continue;
                    }
                }
                if !collect_let_lambda_sites(heap, name, arity, it, scope, sites) {
                    return false;
                }
            }
            true
        }
        _ => true,
    }
}
