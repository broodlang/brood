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
                scope = crate::types::check::sigs::bind_head(heap, scope, h, tys.get(i).cloned());
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
        scope = scope.bind(p, tys.get(i).cloned());
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
    let defaults = fn_param_defaults(heap, params_form);
    // Whether the param list ends in a `& rest` binder (always the last binder). Its
    // seeded type differs — the binder collects the variadic args into a *list*.
    let has_rest = params_form_has_rest(heap, params_form);
    // The closure's actual param count must fall inside the declared sig's
    // arity range for seeding to make sense: at least `params.len()`
    // required, at most `params.len() + optional.len()` unless it has a
    // rest tail (any count at or above `params.len()` is then fine).
    let sig = sig.filter(|s| {
        params.len() >= s.params.len()
            && (s.rest.is_some() || params.len() <= s.params.len() + s.optional.len())
    });
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        // An `&optional` position may genuinely be absent at the call site
        // (bound to `nil`, same as an unsupplied optional with no default) —
        // widen with `nil` and seed it as a plain (not sig-authoritative)
        // type, so a defensive `(nil? p)` in the body is never mistaken for
        // dead code the way an exact required-param contract would be.
        // The `& rest` binder (always last) collects the variadic arguments into a
        // list, so its type is `list<rest-elem>` — not the element type the sig's
        // rest position carries. Seeding it as the bare element type was a false-
        // positive source: `(defn f (& xs) (reduce xs 0 +))` with `(sig f (& int ->
        // …))` would type `xs` as `int` and then flag `(reduce … xs)` for passing an
        // int where a sequence is wanted. Bind it plainly (not sig-authoritative) so
        // no dead-clause lint keys off it.
        if has_rest && i + 1 == params.len() {
            let rest_ty = sig.and_then(|s| s.rest.clone()).map(Ty::list_of);
            scope = scope.bind(p, rest_ty);
            continue;
        }
        let is_optional_pos =
            sig.is_some_and(|s| i >= s.params.len() && i < s.params.len() + s.optional.len());
        match sig.and_then(|s| s.param(i)) {
            Some(ty) if is_optional_pos => {
                // An unsupplied optional WITH a default `(n 1)` is bound to the default,
                // never to nil — so the absence case is the default's type. Seeding it as
                // `T | nil` made `(+ p n)` under `&optional (n 1)` read `nil | int` and
                // every such site a strict warning. A default whose type can't be pinned
                // keeps the nil reading (a superset — sound).
                let absent = defaults
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|d| expr_ty(heap, d, ctx))
                    .unwrap_or(Ty::of(crate::core::value::Tag::Nil));
                scope = scope.bind(p, Some(ty.union(absent)));
            }
            Some(ty) => scope = scope.bind_sig_param(p, ty),
            None => scope = scope.bind(p, None),
        }
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
            let then_ctx = ctx.narrow(g.sym, g.ty.clone());
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
                ctx.narrow(g.sym, g.ty.negate())
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
    // the then-branch (a truthy `and` proves all of them), and an `or`-test whose disjuncts
    // are all biconditional guards over one variable narrows both branches (then → the
    // union, else → its complement). All via intersecting `narrow`, so they compose with
    // the single-guard and path narrowings above. Guards read against the original `ctx`
    // (for let-alias resolution); the tightening lands on the branch ctxs.
    let (then_ctx, else_ctx) = {
        let mut t = then_ctx;
        let mut e = else_ctx;
        for g in and_conjunct_guards(heap, test, ctx) {
            t = t.narrow(g.sym, g.ty);
        }
        if let Some((sym, union)) = or_same_var_narrowing(heap, test, ctx) {
            t = t.narrow(sym, union.clone());
            e = e.narrow(sym, union.negate());
        }
        (t, e)
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
    let mut i = 0;
    while i < binds.len() {
        let Value::Sym(name) = binds[i] else {
            // Destructuring binder (`(let ((a b) rhs) …)`): we can't pin a precise
            // type per position here, but the pattern's symbol leaves ARE bound in
            // the body — bind each to `None` (in scope, unknown type) so a use like
            // `(+ a b)` doesn't misfire as an unbound-symbol error. Still check the
            // RHS as an evaluated expression.
            check_value_leaf(heap, binds[i + 1], form, &scope, out);
            check_into(heap, binds[i + 1], &scope, out);
            let rhs_ty = expr_ty(heap, binds[i + 1], &scope);
            for (sym, ty) in pattern_bindings(heap, binds[i], rhs_ty.as_ref()) {
                scope = scope.bind(sym, ty);
            }
            i += 2;
            continue;
        };
        let rhs = binds[i + 1];
        // The RHS is an evaluated value position — a bare unbound symbol there
        // (`(let (x typo) …)`) is a reference error.
        check_value_leaf(heap, rhs, form, &scope, out);
        check_into(heap, rhs, &scope, out);
        let rhs_ty = expr_ty(heap, rhs, &scope);
        // Is the RHS *precise* (non-redefinable)? Computed in the pre-bind scope.
        // `dynamic == false` ⇔ a literal / integer-closed expression, never a
        // call-result or global reference — the reload-safe subset the dead-clause
        // lint may key off (ADR-131).
        let rhs_precise = !gradual_of(heap, rhs, &scope).dynamic;
        let rhs_guard = guard_assertion(heap, rhs, &scope);
        scope = scope.bind(name, rhs_ty.clone());
        // Dead-clause lint eligibility: a surface (non-gensym), precisely-typed
        // `let`-local joins the set the dead-clause lint may flag, so a later guard
        // that narrows it to `never` is caught — `(let (x 5) (cond (string? x) …))`.
        if rhs_precise
            && rhs_ty.as_ref().is_some_and(|t| !t.is_never())
            && !is_gensym_sym(name)
            && heap.form_pos_only(form).is_some()
            && !heap.is_synthetic(form)
        {
            scope.mark_dead_clause_local(name);
        }
        // Only alias a *biconditional* guard: a `then_only` guard (the `and`
        // short-circuit) must not be stored as a let-alias, or a later
        // `(if alias …)` would negate it in the else-branch (unsound).
        if let Some(g) = rhs_guard {
            if !g.then_only {
                scope = scope.add_guard(name, g.sym, g.ty);
            }
        }
        // A plain `(let (name other) …)` aliases `name` to `other` — narrowing
        // either propagates to the other via `narrow_chain`. This is what
        // makes the `match` pattern compiler's `(let (m__28 x) (if (%eq m__28
        // lit) …))` expansion narrow the user's `x`, not just the internal
        // `m__28`. We don't gate on `other` being a known local: it might be
        // a free reference (e.g. when checking a bare form via
        // `(check 'form)`) or a top-level global — either way, narrowing
        // inside the branch is sound (it describes "if this branch is
        // reached, then…", vacuously true on unreachable paths).
        if let Value::Sym(target) = rhs {
            scope = scope.add_alias(name, target);
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
