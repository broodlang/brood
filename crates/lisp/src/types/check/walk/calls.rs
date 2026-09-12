//! Call-site checking: the per-argument gradual relation, callback arity and signature
//! synthesis for the higher-order combinators (ADR-078), overload clause matching, and
//! the `arg_ty_at` query the LSP arms to ask what type an argument was given.

use super::*;

thread_local! {
    /// The armed [`arg_ty_at`](super::arg_ty_at) query, if any — the position-keyed
    /// type capture behind the LSP's record-field completion. Keyed by the *call
    /// form's* reader `Pos` (not the argument's own) because the interesting
    /// argument is typically a bare symbol, and the form-pos table is pair-keyed —
    /// a symbol carries no position of its own. Captured in [`check_into`]'s walk,
    /// where the scope `Ctx` (let-bound RHS types, sig-typed params, narrowings)
    /// is in force at exactly that point.
    static ARG_TY_QUERY: std::cell::RefCell<Option<ArgTyQuery>> =
        const { std::cell::RefCell::new(None) };
}

pub(in crate::types::check) struct ArgTyQuery {
    line: u32,
    col: u32,
    arg_index: usize,
    result: Option<Ty>,
}

/// Arm the [`ArgTyQuery`] for the next [`check_into`] walk on this thread.
pub(in crate::types::check) fn arm_arg_ty_query(line: u32, col: u32, arg_index: usize) {
    ARG_TY_QUERY.with(|q| {
        *q.borrow_mut() = Some(ArgTyQuery {
            line,
            col,
            arg_index,
            result: None,
        })
    });
}

/// Disarm the query and return whatever type it captured.
pub(in crate::types::check) fn take_arg_ty_query() -> Option<Ty> {
    ARG_TY_QUERY
        .with(|q| q.borrow_mut().take())
        .and_then(|q| q.result)
}

/// The capture hook — called on every list form the walk visits. When the armed
/// query names this form (by reader position) and no type has been captured yet,
/// infer the requested item's type in the scope `ctx` in force here. Macro
/// expansion can duplicate a position (`rebuild_list` copies it), so a `None`
/// capture stays open for a later matching form rather than pinning the miss.
pub(super) fn capture_arg_ty(heap: &Heap, form: Value, items: &[Value], ctx: &Ctx) {
    ARG_TY_QUERY.with(|q| {
        let mut q = q.borrow_mut();
        let Some(query) = q.as_mut() else { return };
        if query.result.is_some() {
            return;
        }
        let matches = heap
            .form_pos_only(form)
            .is_some_and(|p| p.line == query.line && p.col == query.col);
        if !matches {
            return;
        }
        if let Some(&arg) = items.get(query.arg_index) {
            query.result = expr_ty(heap, arg, ctx);
        }
    });
}

/// The finding position for a call **argument**: the argument's own source
/// position when it has one (a nested call / vector — the reader positions
/// pairs, so `(string/length (+ 1 2))` anchors the type finding at `(+ 1 2)`,
/// not the call head), falling back to the whole call form for a bare literal
/// or symbol (which the pair-keyed position table doesn't record). Finer LSP /
/// `nest check` spans without threading `Pos` through the whole walk.
pub(super) fn arg_pos(heap: &Heap, arg: Value, form: Value) -> Option<crate::error::Pos> {
    heap.form_pos_only(arg).or_else(|| heap.form_pos_only(form))
}

/// The arity of a callback argument, when it can be determined *unambiguously* —
/// the input to the callback-arity check (ADR-078). A named **global** function
/// (its arity lives in the heap) or a simple single-clause lambda literal yields
/// an arity; a local variable (arity unknown here), a multi-clause / pattern /
/// variadic lambda, or any non-function form yields `None` (skip — so the check
/// never produces a false positive).
pub(super) fn callback_arity(heap: &Heap, arg: Value, ctx: &Ctx) -> Option<Arity> {
    match arg {
        // A local binding shadows the global table — its arity isn't known here.
        Value::Sym(s) if ctx.is_local(s) => None,
        Value::Sym(s) => arity_of(heap, s),
        Value::Pair(_) => lambda_literal_arity(heap, arg),
        _ => None,
    }
}

/// The signature of a callback **argument**, when one is knowable: a named global's
/// (declared, primitive, curated, or inferred — see [`sigs::sig_of`]), or a file-local
/// one this check has inferred. `None` for a lexical local (the global table doesn't
/// describe it) and for a lambda literal — see [`lambda_sig_under`], which types one
/// against the arrow it is being handed to.
pub(super) fn callback_sig(heap: &Heap, arg: Value, ctx: &Ctx) -> Option<Sig> {
    match arg {
        Value::Sym(s) if ctx.is_lexical_local(s) => None,
        Value::Sym(s) => ctx
            .declared_sig(s)
            .or_else(|| ctx.inferred_fn_sig(s))
            .or_else(|| sig_of(heap, s)),
        _ => None,
    }
}

/// A lambda literal's signature **under the arrow it is being passed as**: its
/// parameters are taken to be exactly `expected`'s (a literal declares no domain of its
/// own, and this is what it will be called with), and its result is the body typed with
/// those inputs — [`infer::callback_ret`], the same inference the HOF rules use.
///
/// This is what closed the gap where a lambda callback was never checked at all:
/// `callback_sig` answered `None` for a literal, so `(g (fn (x) (str x)))` against
/// `(sig g ((int -> int) -> int))` was silent. Typing the body under the DECLARED domain
/// is also what keeps it sound: with `x : int`, `(+ x 1)` is `int` (integer-closed) and
/// passes, where an `(any -> number)` arrow compared by `⊆` would have false-positived.
/// The caller then applies its disjointness rule to the result, which tolerates the
/// over-approximation an inferred return carries. `None` when the body cannot be typed.
pub(super) fn lambda_sig_under(heap: &Heap, arg: Value, expected: &Sig, ctx: &Ctx) -> Option<Sig> {
    if !matches!(arg, Value::Pair(_)) {
        return None;
    }
    let inputs: Vec<Option<Ty>> = expected.params.iter().cloned().map(Some).collect();
    let ret = crate::types::check::infer::callback_ret(heap, arg, &inputs, ctx)?;
    Some(Sig::new(expected.params.clone(), ret))
}

/// The arity of a **single-clause** `fn` literal — `(fn (a b) …)` → `exact(2)`,
/// `(fn (a &optional b) …)` → `range(1, 2)`, `(fn (a b & c) …)` → `at_least(2)`.
/// This mirrors what `arity_of` already computes for a *named* variadic global, so
/// a variadic inline lambda whose *minimum* arity exceeds what a fixed-arity HOF
/// supplies (e.g. `(map (fn (a b & c) …) xs)` — needs ≥2, gets 1) is now caught,
/// while a permissive `(fn (& xs) …)` (math/min 0) still isn't flagged.
///
/// `None` for anything we can't read off cleanly — a multi-arity `fn` (clause
/// lists, not a bare param list), a destructuring parameter, an out-of-order
/// marker, or a non-`fn` head — so the callback-arity check stays
/// false-positive-free.
pub(super) fn lambda_literal_arity(heap: &Heap, form: Value) -> Option<Arity> {
    let items = list_items(heap, form)?;
    let Some(Value::Sym(head)) = items.first().copied() else {
        return None;
    };
    if !is_fn_head(head) {
        return None;
    }
    // Peel an optional leading docstring, matching the evaluator's `fn` parse.
    let parts = &items[1..];
    let parts = match parts.first() {
        Some(Value::Str(_)) if parts.len() > 1 => &parts[1..],
        _ => parts,
    };
    // The parameter list. A multi-arity `fn` has clause *lists* here instead
    // (`((a) …) ((a b) …)`), whose elements aren't bare symbols → we bail below.
    let params = list_items(heap, *parts.first()?)?;
    // Phase machine over the param list: required names, then an optional run
    // after `&optional`, then a single rest binder after `&`. A marker out of
    // order (or repeated) is a shape we don't model — bail.
    #[derive(PartialEq)]
    enum Phase {
        Required,
        Optional,
        Rest,
    }
    let mut phase = Phase::Required;
    let mut required = 0usize;
    let mut optional = 0usize;
    let mut has_rest = false;
    for p in params {
        let Value::Sym(sym) = p else {
            // A destructuring pattern (nested list/vector) or a clause list →
            // not a simple parameter, so not a shape we count here.
            return None;
        };
        if value::symbol_is(sym, kw::AMP_OPTIONAL) {
            if phase != Phase::Required {
                return None;
            }
            phase = Phase::Optional;
        } else if value::symbol_is(sym, kw::AMP) {
            if phase == Phase::Rest {
                return None;
            }
            phase = Phase::Rest;
            has_rest = true;
        } else {
            match phase {
                Phase::Required => required += 1,
                Phase::Optional => optional += 1,
                Phase::Rest => {} // the single rest binder — name doesn't affect arity
            }
        }
    }
    Some(if has_rest {
        Arity::at_least(required)
    } else if optional > 0 {
        Arity::range(required, required + optional)
    } else {
        Arity::exact(required)
    })
}

/// How a callback argument reads in a diagnostic — a named function by its name, an
/// inline one as "the fn" (it said "the lambda" until ADR-162 retired that spelling;
/// a diagnostic shouldn't name a form the language no longer has).
pub(super) fn callback_desc(arg: Value) -> String {
    match arg {
        Value::Sym(s) => name_of(s),
        _ => "the fn".to_string(),
    }
}

/// The output sinks the **function-as-value** lint guards. Passing a bare
/// zero-arg function to one of these stringifies the *function* (`#<fn …>`)
/// instead of calling it — the silent `(print ansi-clear)`-for-`(print
/// (ansi-clear))` slip. Four lock-free `symbol_is` compares, only reached on
/// the generic-call path (so no `symbol_name` allocation on the hot path).
pub(super) fn is_output_sink(s: Symbol) -> bool {
    value::symbol_is(s, "print")
        || value::symbol_is(s, "println")
        || value::symbol_is(s, "str")
        || value::symbol_is(s, "format")
}

/// The arity a signature describes: `&` rest → unbounded, `&optional` → a range, else an
/// exact count. Shared by the declared-sig path and the arrow-parameter path, which have to
/// agree — they are the same question asked of the same shape.
pub(super) fn arity_of_sig(sg: &Sig) -> Arity {
    if sg.rest.is_some() {
        Arity::at_least(sg.params.len())
    } else if sg.optional.is_empty() {
        Arity::exact(sg.params.len())
    } else {
        Arity::range(sg.params.len(), sg.params.len() + sg.optional.len())
    }
}

/// Relax a parameter type for the call-argument membership test, in the two places
/// the type lattice deliberately under-approximates — so the advisory arg-check never
/// misfires on a value that is in fact valid:
///  - a **record-shape** parameter (`(record …)`) drops its **optional** fields,
///    keeping only the required ones. The shape-subtype relation is conservative: a
///    literal `{name}` isn't a subtype of `{name, age?}` even though the value
///    satisfies it (the optional `age` is simply absent), so requiring the optional
///    field's *declaration* would false-flag a valid argument. Dropping optionals
///    keeps the sound part — a missing or wrong-typed *required* field is still caught
///    (so a guard-refined record still flows a real conflict into the call).
///  - a **`list<T>`** parameter also admits the empty list, which the lattice stores
///    as the separate `nil` tag (`Ty::list_of` is `pair`-only by design), so a `nil`
///    argument — the empty list — is consistent with it.
///  - a **callable** parameter (a `(… -> …)` arrow, or the bare `fn | native` of
///    `apply`) also admits a **keyword**, because a keyword IS callable as an
///    accessor (ADR-165): `(map :name people)` is valid, and the lattice has no way
///    to say "keyword, which behaves as (map any ->)" — `Tag::Keyword` and the
///    function tags are disjoint bits. Without this the single most-motivating call
///    for that feature would draw a warning.
pub(super) fn relax_param_for_arg(param: &Ty) -> Ty {
    use crate::types::Tag;
    let mut p = param.clone();
    if p.contains_tag(Tag::Fn) || p.contains_tag(Tag::Native) {
        p = p.union(Ty::of(Tag::Keyword));
    }
    // (A record relaxation used to live here: an expected shape's *optional* fields
    // were dropped before the membership test, because the old subtyping rule refused
    // `{a: 1} <: {a: int, b?: string}` — it would not reason about a field the argument
    // does not declare. Since ADR-264 a shape says what an undeclared key holds, so
    // that comparison is answered directly and correctly; keeping the relaxation would
    // now *cause* the false positive it was written to prevent, by rebuilding the
    // expectation as a CLOSED shape with the optional fields removed.)
    if p.contains_tag(Tag::Pair) && p.elem_ty().is_some() {
        p = p.union(Ty::of(Tag::Nil));
    }
    p
}

/// Does `sig`'s arity accept exactly `argc` arguments — its fixed params, plus
/// any `&optional` slots, plus an unbounded `&rest` tail?
pub(super) fn sig_accepts_argc(sig: &crate::types::Sig, argc: usize) -> bool {
    let min = sig.params.len();
    if argc < min {
        return false;
    }
    sig.rest.is_some() || argc <= min + sig.optional.len()
}

/// ADR-116 completion: does a call with these argument types match **no** arm of
/// a declared overload? False-positive-free by construction — it rules an arm
/// out only when a *known* argument is provably **disjoint** from that arm's
/// parameter (an unknown or `NEVER` arg never rules an arm out), and flags only
/// when *every* arity-relevant arm is ruled out. Arms whose arity can't accept
/// `argc` are left to the separate arity check (so a pure arity mismatch isn't
/// double-reported); if no arm even has a fitting arity we defer entirely.
pub(super) fn overload_arg_mismatch(sigs: &[crate::types::Sig], arg_tys: &[Option<Ty>]) -> bool {
    let argc = arg_tys.len();
    let mut any_arity_ok = false;
    for sig in sigs {
        if !sig_accepts_argc(sig, argc) {
            continue;
        }
        any_arity_ok = true;
        let possible = arg_tys.iter().enumerate().all(|(i, arg_ty)| match arg_ty {
            // An unknown arg, or a `NEVER` (unreachable-branch) arg, never rules
            // an arm out — matches the single-sig loop's `is_never` skip.
            Some(a) if !a.is_never() => sig.param(i).is_none_or(|p| !a.is_disjoint(&p)),
            _ => true,
        });
        if possible {
            return false; // some arity-relevant arm could accept the call
        }
    }
    any_arity_ok // ≥1 arm had a fitting arity, and every such arm was ruled out
}

/// The parameter lists of the clauses a call's *arity* selects, rendered for the
/// mismatch diagnostic — `(string), (int)`. Without it the warning says only that
/// nothing matched, leaving the reader to work out what would have.
pub(super) fn clause_domains_desc(sigs: &[crate::types::Sig], argc: usize) -> String {
    let rendered: Vec<String> = sigs
        .iter()
        .filter(|sig| sig_accepts_argc(sig, argc))
        .map(|sig| {
            let params: Vec<String> = sig.params.iter().map(Ty::to_string).collect();
            format!("({})", params.join(", "))
        })
        .collect();
    rendered.join(", ")
}

/// True when a call whose head is `s` *evaluates its arguments as values* — `s`
/// resolves to a primitive, a known Brood closure, a curated stdlib fn, or a
/// lexical local (a param / `let` name, never a macro). False for a macro, a
/// special-form keyword, an unknown head, or anything we can't prove is a
/// non-macro callable.
///
/// This gates the operand-unbound check: only when arguments are genuinely
/// evaluated is a bare-symbol operand a *reference* (so an unresolvable one is
/// truly unbound). For a macro or unknown head the operands may be opaque syntax
/// (pattern keywords, quoted tags) or a forward reference, so they're left
/// untouched — preserving the checker's no-false-positives rule.
pub(super) fn evaluates_args(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_lexical_local(s) {
        return true;
    }
    match crate::types::check::deps::obs_global(heap, s) {
        Some(Value::Native(_)) | Some(Value::Fn(_)) => true,
        // A `Value::Macro` does NOT evaluate its args; any other bound non-callable
        // isn't a call we should reason about either.
        Some(_) => false,
        // Not in the heap: only the curated stdlib closures count as known callables.
        None => curated_sig(s).is_some(),
    }
}

/// For `(fold coll init f)` / `(reduce coll init f)` / `(reduce f coll)` whose `f` is a
/// two-parameter `fn` literal: `(1, (acc elem -> any))` — the seed for walking it. `None`
/// when the head is neither, `f` is not such a literal, or the fold's type is unknown.
pub(super) fn fold_callback_seed(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
) -> Option<(usize, Sig)> {
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    let is_fold = value::symbol_is(head, "fold") || value::symbol_is(head, "reduce");
    if !is_fold || items.len() < 3 {
        return None;
    }
    // Positions come from `sigs::combinator_args` — the one place the data-first
    // convention is stated (ADR-308), so this cannot drift against the signatures.
    let (coll_arg, f) = crate::types::check::sigs::combinator_args(items)?;
    let f_items = list_items(heap, f)?;
    if !matches!(f_items.first(), Some(&Value::Sym(h)) if is_fn_head(h)) {
        return None;
    }
    if !matches!(lambda_literal_arity(heap, f), Some(a) if a.min == 2 && a.max == Some(2)) {
        return None;
    }
    let acc = expr_ty(heap, form, ctx)?;
    let elem = expr_ty(heap, coll_arg, ctx)
        .and_then(|t| t.elem_ty())
        .unwrap_or(Ty::ANY);
    Some((items.len() - 1, Sig::new(vec![acc, elem], Ty::ANY)))
}

/// How many arguments a CLAUSE-form `fn` literal takes, when every clause agrees — the
/// clause-shaped sibling of [`lambda_literal_arity`], which bails on this form. `None` when
/// the literal is not clause-shaped, has no clauses, or its clauses disagree (a genuinely
/// multi-arity callback, which no combinator calls at two arities anyway).
pub(super) fn clause_literal_takes(heap: &Heap, f: Value) -> Option<usize> {
    let items = list_items(heap, f)?;
    if !matches!(items.first(), Some(&Value::Sym(h)) if is_fn_head(h)) {
        return None;
    }
    let parts = &items[1..];
    let parts = match parts.first() {
        Some(Value::Str(_)) if parts.len() > 1 => &parts[1..],
        _ => parts,
    };
    if parts.is_empty()
        || !parts
            .iter()
            .all(|&c| crate::eval::macros::is_clause(heap, c))
    {
        return None;
    }
    let mut takes: Option<usize> = None;
    for &clause in parts {
        let heads = list_items(heap, list_items(heap, clause)?.first().copied()?)?;
        match takes {
            None => takes = Some(heads.len()),
            Some(n) if n == heads.len() => {}
            Some(_) => return None,
        }
    }
    takes
}

/// The element-consuming combinators: their callback takes ONE argument, the element.
/// Only names whose callback is called with exactly the element — `sort-by`'s key fn,
/// `group-by`'s classifier, `map`'s transform — never one that is handed an index or a
/// pair alongside it.
pub(super) const ELEMENT_CALLBACK_COMBINATORS: &[&str] = &[
    "map",
    "mapv",
    "mapcat",
    // Both spellings: the qualified one is how `seq/` names are written outside a
    // `(:use seq)` module, the bare one how they read inside it. Unlike the curated
    // signature table, a key here suppresses no lint — it only seeds a callback's
    // parameter type — so carrying both costs nothing and covers both call styles.
    "filter",
    "seq/filter",
    "reject",
    "seq/reject",
    "keep",
    "seq/keep",
    "each",
    "take-while",
    "drop-while",
    "sort-by",
    "group-by",
    "any?",
    "all?",
    "none?",
    "count-if",
    "find",
];

/// For `(map coll f)` and its siblings whose `f` is a ONE-parameter `fn` literal:
/// `(1, (elem -> any))` — the seed for walking it.
///
/// The fold seed above existed; these did not, so a callback param bound `any` and its body
/// went unchecked: `(map ["s"] (fn (p) (+ p 1)))` was silent, because `any` is consistent
/// with `number`. The element type is exactly what the combinator promises to hand over, so
/// binding it is the same move `fold_callback_seed` already makes for the accumulator.
///
/// Bound as an INFERRED type (`check_fn_bound`, `dynamic_within`) rather than a declared
/// one, for the reason that function's own comment gives: an element type read by inclusion
/// would flag correct code where the collection's type is an over-approximation.
pub(super) fn element_callback_seed(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
) -> Option<(usize, crate::types::Sig)> {
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    let name = name_of(head);
    if !ELEMENT_CALLBACK_COMBINATORS.contains(&name.as_str()) || items.len() < 3 {
        return None;
    }
    // Same source of truth for argument order as the fold seed (ADR-308).
    let (coll_arg, f) = crate::types::check::sigs::combinator_args(items)?;
    let f_items = list_items(heap, f)?;
    if !matches!(f_items.first(), Some(&Value::Sym(h)) if is_fn_head(h)) {
        return None;
    }
    // Arity 1, however the literal is written. `lambda_literal_arity` deliberately bails on
    // the CLAUSE form (its parts are clause lists, not bare symbols), so a clause callback
    // — the shape people reach for the moment they want to match on the element — got no
    // seed at all. Accept it when every clause takes exactly one head.
    let takes_one = matches!(lambda_literal_arity(heap, f), Some(a) if a.min == 1 && a.max == Some(1))
        || clause_literal_takes(heap, f) == Some(1);
    if !takes_one {
        return None;
    }
    let elem = expr_ty(heap, coll_arg, ctx).and_then(|t| t.elem_ty())?;
    Some((items.len() - 1, crate::types::Sig::new(vec![elem], Ty::ANY)))
}

/// For any call whose callee's signature declares an ARROW at position `i` and whose
/// argument there is a single-clause `fn` literal of that arity: `(i, arrow)` — the
/// seed for walking it. The general case of the two seeds above (2026-09-12): those
/// name their combinators, and every other higher-order function — a `(sig each3
/// (… (buffer int int int -> buffer) -> …))` of the author's own — handed its lambda
/// nothing, so the lambda's parameters inferred from their own body (`(+ start l)` →
/// `number`) and strict reported the very arithmetic the arrow declares as `int`. The
/// arrow's parameters are exactly what the callee promises to hand over (and its body is
/// checked against that promise, ADR-273), so binding them is the same move
/// `element_callback_seed` makes with an element type — and bound the same way, as an
/// INFERRED type, since a declared arrow may over-approximate.
///
/// The callee is resolved as the call check resolves it: a local whose own type is an
/// arrow first, then the file's declaration, the global's signature, the file's
/// inference. First matching position only (the seed API carries one).
pub(super) fn arrow_callback_seed(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
) -> Option<(usize, crate::types::Sig)> {
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    let callee = ctx
        .get(head)
        .as_ref()
        .and_then(Ty::as_arrow)
        .cloned()
        .or_else(|| ctx.declared_sig(head))
        .or_else(|| {
            (!ctx.is_lexical_local(head) && !ctx.is_file_global(head))
                .then(|| sig_of(heap, head))
                .flatten()
        })
        .or_else(|| {
            (!ctx.is_lexical_local(head))
                .then(|| ctx.inferred_fn_sig(head))
                .flatten()
        })?;
    for (i, &arg) in items[1..].iter().enumerate() {
        let Some(param) = callee.param(i) else { break };
        let Some(arrow) = param.as_arrow() else {
            continue;
        };
        if arrow.rest.is_some() {
            continue;
        }
        let Some(f_items) = list_items(heap, arg) else {
            continue;
        };
        if !matches!(f_items.first(), Some(&Value::Sym(h)) if is_fn_head(h)) {
            continue;
        }
        let wanted = arrow.params.len();
        let fits = matches!(lambda_literal_arity(heap, arg), Some(a) if a.min == wanted && a.max == Some(wanted));
        if !fits {
            continue;
        }
        return Some((i + 1, crate::types::Sig::new(arrow.params.clone(), Ty::ANY)));
    }
    None
}

/// The **gradual** type of an expression in *assignment* position — the value
/// flowing into a `(def x …)` whose `x` has a declared value type. This is the
/// first consumer of [`GradualTy`] (ADR-024): the gradual `dynamic()` is what lets
/// the check defer on a redefinable reference instead of fighting hot reload.
///
/// - A **literal** (non-symbol, non-call) has an exact, non-redefinable type →
///   `stat(t)` (checked with `⊆` — sound because the type is precise).
/// - A bare reference to a **redefinable global** is `dynamic`, bounded by its own
///   declared value type when it has one (`dynamic_within(t)`) or pure `dynamic()`
///   otherwise — the *bounded-dynamic* case `Option<Ty>` can't represent, and what
///   lets `(def x g)` be caught when `g`'s declared type is disjoint from `x`'s.
/// - A **local** or a **call result** carries an *over-approximated* type, so it's
///   `dynamic_within(t)` — consistency then uses `∩ ≠ ⊥`, which can't over-warn on a
///   widened type (a number-returning call assigned to an `int` slot defers, not
///   warns). Unknown → pure `dynamic()` (always consistent — defer).
pub(super) fn gradual_of(heap: &Heap, expr: Value, ctx: &Ctx) -> GradualTy {
    if let Value::Sym(s) = expr {
        // A known/narrowed type for `s` in the current scope — a fn param, a let
        // binding, OR a *guard narrowing* on any variable (a narrowing lands in
        // `ctx.get`, whether or not `s` is a lexical local, so this must be checked
        // for every symbol — gating it on `is_lexical_local` dropped a narrowing on
        // a free variable). A `(sig …)`-seeded param carries its *exact* contract
        // type → `stat` (precise, `⊆`): using it where a narrower type is wanted is
        // a real mismatch. Anything else (a `let` local whose RHS was a call, a
        // guard-narrowed variable) is an over-approximation bound → `dynamic_within`
        // (the `∩` relation, which never over-warns on a merely-wider type).
        if let Some(t) = ctx.get(s) {
            // `any` is the exception: a param declared `any` (e.g. `(sig set (any ->
            // set))` for "any seqable") carries *no* constraint — it is the gradual
            // "unknown", not a precise top type. Treating it as `stat(ANY)` would then
            // fail a `⊆` test against any narrower param (`(fold … coll)` wants a
            // collection), a false positive. So `any` is always `dynamic` — the
            // `dynamic()`-not-`Any` rule, applied to a declared param.
            return if ctx.is_sig_param(s) && !t.is_any() {
                GradualTy::stat(t)
            } else {
                GradualTy::dynamic_within(t)
            };
        }
        // A lexical local with no known type is in scope but unknown → `dynamic()`.
        if ctx.is_lexical_local(s) {
            return GradualTy::dynamic();
        }
        // Otherwise a (redefinable) global / file-global: dynamic, bounded by its
        // own declared value type when it has one — the bounded-dynamic case.
        // The file-local ctx (a `(sig …)` in *this* file's un-expanded forms)
        // wins; the heap-wide store (`declared_heap_value_ty`) covers a
        // same-module reference that got qualified to `mod/name` during
        // expansion, or a genuine cross-module reference — same fix
        // `declared_heap_sig` already applies for arrows.
        // For a name this file redefines, the heap-wide stores describe the OLD
        // binding (the file is checked pre-load) — only the file-local ctx
        // sources apply (ADR-123: a def always wins).
        let heap_declared = (!ctx.is_file_global(s))
            .then(|| declared_heap_value_ty(heap, s))
            .flatten();
        let heap_global = (!ctx.is_file_global(s))
            .then(|| global_value_ty(heap, s))
            .flatten();
        return match ctx
            .declared_value_ty(s)
            .or(heap_declared)
            .or_else(|| ctx.inferred_value_ty(s))
            .or(heap_global)
        {
            // The Gap A inferred current-image type (same-file `inferred_value_ty`,
            // or cross-file `global_value_ty` read from the loaded image) is exposed
            // as `dynamic_within` like a declared global — the `∩` relation, so a
            // reload that changes it is re-checked, never a stale hard proof.
            Some(t) => GradualTy::dynamic_within(t),
            None => GradualTy::dynamic(),
        };
    }
    // A compound form: control-flow forms recurse into their result positions
    // (each one's gradual type, joined), so a body assembled from *precise* pieces
    // (literals, sig-params, integer-closed arithmetic) is `stat` (checked `⊆`,
    // catching a merely-wider body), while any over-approximated call branch makes
    // the join `dynamic` (the ∩-relation, which never over-warns on a widened type).
    if matches!(expr, Value::Pair(_)) {
        if let Some(g) = gradual_of_compound(heap, expr, ctx) {
            return g;
        }
    }
    match expr_ty(heap, expr, ctx) {
        // A bare literal (not a call) is exact → static.
        Some(t) if !matches!(expr, Value::Pair(_)) => GradualTy::stat(t),
        // A call result is an over-approximation → dynamic (∩-relation, no over-warn).
        Some(t) => GradualTy::dynamic_within(t),
        None => GradualTy::dynamic(),
    }
}

/// The gradual type of a *compound* (`Pair`) expression when it's a form whose
/// result we can type **precisely** — a control-flow form (whose value is one of
/// its sub-forms) or the integer-closed arithmetic rule. Recurses into each result
/// position via [`gradual_of`], so the staticness propagates: an all-precise body
/// stays `stat` (warns on a merely-wider type via `⊆`), and any over-approximated
/// call branch makes the join `dynamic` (defers on widening). Returns `None` for
/// any other form (a plain call / unrecognised shape) — the caller then uses the
/// flat `expr_ty` → `dynamic_within` path. Mirrors `guards::control_flow_ty`'s shape
/// but carries the gradual `?` so the return/assignment check stays
/// false-positive-clean.
pub(super) fn gradual_of_compound(heap: &Heap, expr: Value, ctx: &Ctx) -> Option<GradualTy> {
    let items = list_items(heap, expr)?;
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    // A lexical local can shadow a special-form name; then it isn't this form.
    if ctx.is_lexical_local(head) {
        return None;
    }
    // `(if test then else)` → join(then, else); `(if test then)` → then | nil.
    // Narrow each branch by what the test guard asserts, mirroring `check_if` —
    // so `(if (int? x) x 0)` types the then-branch's `x` as `int`, not the
    // declared `number` (the precise return-check would otherwise false-positive).
    if value::symbol_is(head, kw::IF) {
        let test = items.get(1).copied().unwrap_or(Value::nil());
        let (then_ctx, else_ctx) = crate::types::check::guards::branch_scopes(heap, test, ctx);
        // A LITERAL condition selects its branch (only nil/false are falsy; every other
        // literal is truthy), the same fold `infer::control_flow_ty` applies — so the
        // argument check sees `1`, not `1 | "a"`, for `(if true 1 "a")`.
        match items.get(1) {
            Some(Value::Nil) | Some(Value::Bool(false)) => {
                return Some(match items.len() {
                    4 => gradual_of(heap, items[3], &else_ctx),
                    _ => GradualTy::stat(NIL_TY),
                });
            }
            Some(Value::Bool(true))
            | Some(Value::Int(_))
            | Some(Value::Float(_))
            | Some(Value::Keyword(_))
            | Some(Value::Str(_)) => {
                if items.len() == 3 || items.len() == 4 {
                    return Some(gradual_of(heap, items[2], &then_ctx));
                }
            }
            _ => {}
        }
        // A dead branch (its scope contradicted by the test — `Ctx::is_dead`) contributes
        // nothing to the value.
        return match items.len() {
            4 => match (then_ctx.is_dead(), else_ctx.is_dead()) {
                (false, false) => Some(
                    gradual_of(heap, items[2], &then_ctx)
                        .union(gradual_of(heap, items[3], &else_ctx)),
                ),
                (true, false) => Some(gradual_of(heap, items[3], &else_ctx)),
                (false, true) => Some(gradual_of(heap, items[2], &then_ctx)),
                (true, true) => None,
            },
            3 if then_ctx.is_dead() => Some(GradualTy::stat(NIL_TY)),
            3 => Some(gradual_of(heap, items[2], &then_ctx).union(GradualTy::stat(NIL_TY))),
            _ => None,
        };
    }
    // `(do … last)` → gradual(last). Empty `(do)` → nil.
    if value::symbol_is(head, kw::DO) {
        return match items.last() {
            Some(&last) if items.len() > 1 => Some(gradual_of(heap, last, ctx)),
            _ => Some(GradualTy::stat(NIL_TY)),
        };
    }
    // `(when t body…)` / `(unless t body…)` → gradual(last) | nil.
    if value::symbol_is(head, kw::WHEN) || value::symbol_is(head, kw::UNLESS) {
        let &last = items.last()?;
        if items.len() < 3 {
            return Some(GradualTy::stat(NIL_TY));
        }
        return Some(gradual_of(heap, last, ctx).union(GradualTy::stat(NIL_TY)));
    }
    // `let`/`letrec` → gradual(last body), with each binding's RHS type
    // threaded into the scope (so a precise RHS makes its uses precise).
    if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
        let binds = bindings(heap, *items.get(1)?)?;
        if binds.len() % 2 != 0 || items.len() < 3 {
            return None;
        }
        let mut scope = ctx.clone();
        let mut i = 0;
        while i < binds.len() {
            let rhs_ty = expr_ty(heap, binds[i + 1], &scope);
            match binds[i] {
                Value::Sym(name) => scope = scope.bind(name, rhs_ty),
                pat => {
                    for (sym, ty) in pattern_bindings(heap, pat, rhs_ty.as_ref()) {
                        scope = scope.bind(sym, ty);
                    }
                }
            }
            i += 2;
        }
        let &last = items.last()?;
        return Some(gradual_of(heap, last, &scope));
    }
    // `(cond t1 r1 … :else rN)` → join of the result positions (odd index ≥ 2).
    if value::symbol_is(head, kw::COND) {
        let results: Vec<Value> = items[2..].iter().step_by(2).copied().collect();
        return gradual_join(heap, &results, ctx);
    }
    // `(case key v1 r1 … [default])` → join of each pair's result + a lone default.
    if value::symbol_is(head, kw::CASE) && items.len() >= 4 {
        let clauses = &items[2..];
        let mut results = Vec::new();
        let mut i = 0;
        while i < clauses.len() {
            if i + 1 < clauses.len() {
                results.push(clauses[i + 1]);
                i += 2;
            } else {
                results.push(clauses[i]);
                i += 1;
            }
        }
        return gradual_join(heap, &results, ctx);
    }
    // `(match scrut pat1 body1 …)` → join of the arm bodies (even offset from 3).
    if value::symbol_is(head, kw::MATCH) {
        let bodies: Vec<Value> = items[3..].iter().step_by(2).copied().collect();
        return gradual_join(heap, &bodies, ctx);
    }
    // `(and …)` / `(or …)` → join of operands (any can be the short-circuit value).
    if value::symbol_is(head, kw::AND) {
        if items.len() == 1 {
            return Some(GradualTy::stat(Ty::of(value::Tag::Bool)));
        }
        return gradual_join(heap, &items[1..], ctx);
    }
    if value::symbol_is(head, kw::OR) {
        if items.len() == 1 {
            return Some(GradualTy::stat(NIL_TY));
        }
        return gradual_join(heap, &items[1..], ctx);
    }
    // The integer-closed arithmetic rule produces a *precise* `int` (not an
    // over-approximation), so a `(* x x)`-style body declared `int` is `stat` and
    // checked with `⊆` — no false positive (the rule only fires when every operand
    // is a known integer; `guards::expr_ty` routes it through `numeric_call_ty`).
    if let Some(t) = expr_ty(heap, expr, ctx) {
        if is_int_closed_op(head) && t.is_subtype(&Ty::of(value::Tag::Int)) {
            return Some(GradualTy::stat(t));
        }
    }
    None
}

/// `nil` as a `Ty` — the value of an empty/else-less control-flow branch.
pub(super) const NIL_TY: Ty = Ty::of(value::Tag::Nil);

/// Join the gradual types of several branch result forms (the `cond`/`case`/
/// `match`/`and`/`or` arms). `None` when there are no results (an empty clause
/// list — defer rather than invent a type).
pub(super) fn gradual_join(heap: &Heap, forms: &[Value], ctx: &Ctx) -> Option<GradualTy> {
    let mut acc: Option<GradualTy> = None;
    for &f in forms {
        let g = gradual_of(heap, f, ctx);
        acc = Some(match acc {
            Some(a) => a.union(g),
            None => g,
        });
    }
    acc
}

/// Is `head` one of the integer-closed arithmetic ops whose result on integer
/// operands is precisely `int` (mirrors `guards::numeric_call_ty`)? `/` is excluded
/// (int/int can be a float).
pub(super) fn is_int_closed_op(head: Symbol) -> bool {
    value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "quot")
        || value::symbol_is(head, "rem")
        || value::symbol_is(head, "mod")
        // `math/` since ADR-227 — the bare spelling no longer exists, so keying it here
        // left this rule dead for the spelling that does (mirrors `infer.rs`).
        || value::symbol_is(head, "math/abs")
}
