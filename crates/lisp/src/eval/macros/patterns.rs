//! Pattern-binder lowering: the compile pass desugars destructuring `let`/`fn` to `match*`.

use super::*;

// ---- pattern-binder lowering (the compile pass desugars these to `match*`) ----

fn is_sym(v: Value) -> bool {
    matches!(v.unpack(), ValueRef::Sym(_))
}

fn make_do(heap: &mut Heap, body: &[Value]) -> Value {
    let mut v = Vec::with_capacity(body.len() + 1);
    v.push(value::sym(kw::DO));
    v.extend_from_slice(body);
    heap.list(v)
}

/// `(match* :ctx valexpr (pattern inner))` — a single-clause refutable bind.
fn refutable_bind(
    heap: &mut Heap,
    ctx: &str,
    valexpr: Value,
    pattern: Value,
    inner: Value,
) -> Value {
    let clause = heap.list(vec![pattern, inner]);
    heap.list(vec![
        value::sym(kw::MATCH_STAR),
        value::kw(ctx),
        valexpr,
        clause,
    ])
}

/// Lower a `let` whose bindings include a non-symbol (pattern) target into
/// nested symbol-`let` / refutable `match*` binds (sequential, so each sees the
/// previous). Returns `None` for an all-symbol or malformed `let` (left as-is).
pub(super) fn lower_let(heap: &mut Heap, items: &[Value]) -> Option<Value> {
    let bindings = *items.get(1)?;
    let binds = heap.list_or_vector_items(bindings)?;
    if binds.len() % 2 != 0 {
        return None; // malformed: let eval report it
    }
    if !binds.iter().step_by(2).any(|&t| !is_sym(t)) {
        return None; // all targets are plain symbols — ordinary let
    }
    let body = &items[2..];
    let mut acc = make_do(heap, body);
    let mut i = binds.len();
    while i >= 2 {
        let (target, valexpr) = (binds[i - 2], binds[i - 1]);
        acc = if is_sym(target) {
            let bind = heap.list(vec![target, valexpr]);
            heap.list(vec![value::sym(kw::LET), bind, acc])
        } else {
            refutable_bind(heap, kw::LET, valexpr, target, acc)
        };
        i -= 2;
    }
    Some(acc)
}

/// A multi-clause `fn` clause is `(param-list body...)` where the param-list is
/// itself a list (or `()`). A vector head is *not* a clause (param lists are
/// lists, ADR-010) — that disambiguates a single tuple param from a clause.
pub(crate) fn is_clause(heap: &Heap, f: Value) -> bool {
    match f.unpack() {
        ValueRef::Pair(p) => matches!(heap.car(p).unpack(), ValueRef::Pair(_) | ValueRef::Nil),
        _ => false,
    }
}

/// Like [`is_clause`], but *also* true for a **vector**-headed form. Recognised
/// only so it can be rejected: `(defn f ([x] :one) ([x y] :two))` is Clojure's
/// multi-arity spelling, and reading it as Brood would silently produce one
/// 2-parameter function with an empty body (`[x]` and `:one` as two patterns) —
/// a different program, diagnosed only later as a misleading arity error at the
/// call site. See [`crate::eval::vector_binding_container_error`].
fn is_clause_shaped(heap: &Heap, f: Value) -> bool {
    match f.unpack() {
        ValueRef::Pair(p) => matches!(
            heap.car(p).unpack(),
            ValueRef::Pair(_) | ValueRef::Nil | ValueRef::Vector(_)
        ),
        _ => false,
    }
}

/// Does any clause head of an all-clause-shaped `fn` body use the vector
/// spelling? (`clauses` is every form after `fn` and an optional docstring.)
fn has_vector_clause_head(heap: &Heap, clauses: &[Value]) -> bool {
    !clauses.is_empty()
        && clauses.iter().all(|&f| is_clause_shaped(heap, f))
        && clauses.iter().any(|&f| match f.unpack() {
            ValueRef::Pair(p) => matches!(heap.car(p).unpack(), ValueRef::Vector(_)),
            _ => false,
        })
}

/// Is `param_form` an *arity* parameter list — only plain symbols (params) and
/// the `&optional`/`&` markers, with no literal or destructuring *patterns*?
/// Arity clauses dispatch by argument count via native multi-arity arms
/// (`ClosureArm`, cheap — direct bind); a clause with any non-symbol parameter is
/// a *pattern* clause and must go through the `match*` engine instead.
pub(crate) fn is_arity_param_list(heap: &Heap, param_form: Value) -> bool {
    match heap.list_or_vector_items(param_form) {
        Some(items) => items.iter().all(|&p| is_sym(p)),
        None => false,
    }
}

/// A clause whose parameter list is an arity list (see [`is_arity_param_list`]).
pub(crate) fn is_arity_clause(heap: &Heap, f: Value) -> bool {
    match f.unpack() {
        ValueRef::Pair(p) => {
            let head = heap.car(p);
            matches!(head.unpack(), ValueRef::Pair(_) | ValueRef::Nil)
                && is_arity_param_list(heap, head)
        }
        _ => false,
    }
}

/// Does this clause carry a `:when` guard — is the form right after its
/// parameter-list head the `:when` keyword (`(params :when guard body…)`)?
/// A guarded clause must dispatch through `match*` (which evaluates the guard);
/// the native arity path binds by argument count and cannot, so it would silently
/// ignore the guard. See [`lower_fn`] (ADR-226).
pub(crate) fn clause_has_when_guard(heap: &Heap, clause: Value) -> bool {
    heap.list_items(clause)
        .and_then(|parts| parts.get(1).copied())
        .is_some_and(
            |v| matches!(v.unpack(), ValueRef::Keyword(s) if value::symbol_name_ref(s) == "when"),
        )
}

/// What a clause head reaches: the argument counts it accepts, and whether it takes EVERY
/// call of those counts — no `:when` guard, and every slot a pattern that cannot fail (a
/// plain name, `_`, an `&optional` name with or without a default, a named rest).
#[derive(Clone, Copy)]
struct ClauseReach {
    required: usize,
    optional: usize,
    rest: bool,
    total: bool,
}

impl ClauseReach {
    /// Does `self` take every call `later` would? Then `later` can never run.
    fn covers(self, later: ClauseReach) -> bool {
        self.total
            && self.required <= later.required
            && (self.rest
                || (!later.rest
                    && self.required + self.optional >= later.required + later.optional))
    }
}

/// A slot pattern that matches anything: a plain name (`_` included). A name repeated
/// within the head requires equality, so it is not total.
fn total_slot(p: Value, seen: &mut Vec<value::Symbol>) -> bool {
    let ValueRef::Sym(s) = p.unpack() else {
        return false;
    };
    let repeated = !value::symbol_is(s, "_") && seen.contains(&s);
    seen.push(s);
    !repeated
}

fn clause_reach(heap: &Heap, clause: Value) -> Option<ClauseReach> {
    let head = match clause.unpack() {
        ValueRef::Pair(p) => heap.car(p),
        _ => return None,
    };
    let items = match head.unpack() {
        ValueRef::Nil => Vec::new(),
        _ => heap.list_items(head)?,
    };
    let mut reach = ClauseReach {
        required: 0,
        optional: 0,
        rest: false,
        total: !clause_has_when_guard(heap, clause),
    };
    let mut seen = Vec::new();
    let mut section = 0; // 0 required, 1 &optional, 2 rest
    for p in items {
        if let ValueRef::Sym(s) = p.unpack() {
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                section = 1;
                continue;
            }
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST) {
                section = 2;
                reach.rest = true;
                continue;
            }
        }
        let slot = match section {
            0 => {
                reach.required += 1;
                p
            }
            // `name`, `(name default)` or `(pattern default)`: the slot's pattern is its first
            1 => {
                reach.optional += 1;
                match p.unpack() {
                    ValueRef::Pair(c) => heap.car(c),
                    _ => p,
                }
            }
            _ => p,
        };
        if !total_slot(slot, &mut seen) {
            reach.total = false;
        }
    }
    Some(reach)
}

/// The error for the first clause an earlier clause shadows, if any. Clauses are tried in
/// order and the first that matches runs (ADR-619), so a clause whose every call an earlier
/// one takes is dead code — refused rather than silently never run, which is what keeps
/// clause order from changing a program's meaning unnoticed.
pub(crate) fn shadowed_clause_error(heap: &Heap, clauses: &[Value]) -> Option<LispError> {
    let reaches: Vec<Option<ClauseReach>> =
        clauses.iter().map(|&c| clause_reach(heap, c)).collect();
    for (j, later) in reaches.iter().enumerate() {
        let Some(later) = *later else { continue };
        for (i, earlier) in reaches[..j].iter().enumerate() {
            let Some(earlier) = *earlier else { continue };
            if earlier.covers(later) {
                let head_of = |c: Value| match c.unpack() {
                    ValueRef::Pair(p) => crate::syntax::printer::print(heap, heap.car(p)),
                    _ => crate::syntax::printer::print(heap, c),
                };
                let (shown_j, shown_i) = (head_of(clauses[j]), head_of(clauses[i]));
                return Some(
                    LispError::runtime(format!(
                        "fn: the clause {shown_j} can never run: the clause {shown_i} before it \
                         takes every call it would"
                    ))
                    .with_hint(
                        "clauses are tried in order and the first that matches runs (ADR-619). \
                         Put the more specific clause first, or delete the dead one.",
                    ),
                );
            }
        }
    }
    None
}

/// `(%raise (%match-error :fn ARGS '(HEAD)))` for a single clause whose guard failed: ARGS
/// the arguments as a list, rebuilt from the native parameters (`(append (list a b) rest)`
/// with a rest), as the `match*` a clause `fn` lowers to would report them.
fn guard_failure(heap: &mut Heap, params: &[Value], head: Value) -> Value {
    let mut fixed = vec![value::sym("list")];
    let mut rest = None;
    let mut section = 0; // 0 required, 1 &optional, 2 rest
    for &p in params {
        if let ValueRef::Sym(s) = p.unpack() {
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                section = 1;
                continue;
            }
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST) {
                section = 2;
                continue;
            }
        }
        match (section, p.unpack()) {
            (2, _) => rest = Some(p),
            (1, ValueRef::Pair(c)) => fixed.push(heap.car(c)),
            _ => fixed.push(p),
        }
    }
    let fixed = heap.list(fixed);
    let args = match rest {
        Some(r) => heap.list(vec![value::sym("append"), fixed, r]),
        None => fixed,
    };
    let quoted_head = heap.list(vec![head]);
    let patterns = heap.list(vec![value::sym(kw::QUOTE), quoted_head]);
    let error = heap.list(vec![
        value::sym("%match-error"),
        value::kw("fn"),
        args,
        patterns,
    ]);
    heap.list(vec![value::sym("%raise"), error])
}

/// Is `body` a single clause's `:when guard rest…`? The guard and the forms after it.
fn single_clause_guard(body: &[Value]) -> Option<(Value, &[Value])> {
    match body {
        [w, guard, rest @ ..] if matches!(w.unpack(), ValueRef::Keyword(s) if value::symbol_name_ref(s) == "when") => {
            Some((*guard, rest))
        }
        _ => None,
    }
}

/// Does a single-clause `fn` need the `match*` lowering rather than native parameters — a
/// pattern in an `&optional` slot (`([a b] [0 0])`, or a bare `[a b]`), or a pattern after
/// `&`? (A guard does not: it wraps the native body, so the checker keeps the parameters.)
fn single_clause_needs_match(heap: &Heap, params: &[Value]) -> bool {
    let mut section = 0; // 0 required, 1 &optional, 2 rest
    let mut pattern_slot = false;
    for &p in params {
        if let ValueRef::Sym(s) = p.unpack() {
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                section = 1;
                continue;
            }
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST) {
                section = 2;
                continue;
            }
        }
        pattern_slot |= match section {
            0 => false,
            // `name` and `(name default)` are native; anything else is a pattern
            1 => match p.unpack() {
                ValueRef::Sym(_) => false,
                ValueRef::Pair(c) => !is_sym(heap.car(c)),
                _ => true,
            },
            _ => !is_sym(p),
        };
    }
    pattern_slot
}

/// Cheap predicate: does this `fn`/`lambda` form need pattern lowering — i.e. is
/// it multi-clause, or single-clause with a pattern in a required parameter?
/// Mirrors [`lower_fn`]'s dispatch. Used by the evaluator as a fallback for `fn`
/// forms that reached it without the compile pass (built by a quasiquote, or a
/// macro expanded lazily within its defining form); an ordinary `fn` returns
/// `false` here and takes the normal `make_closure` path.
pub(crate) fn fn_needs_lowering(heap: &Heap, fn_form: Value) -> bool {
    let items = match heap.list_items(fn_form) {
        Some(items) => items,
        None => return false,
    };
    let forms = &items[1..];
    // Peel a leading docstring (matches `lower_fn`), so a multi-clause fn with a
    // docstring is still recognised as needing lowering.
    let forms = match forms.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    if forms.is_empty() {
        return false;
    }
    if forms.iter().all(|&f| is_clause(heap, f)) {
        // Multi-clause. Arity-only clauses dispatch natively (`make_closure`
        // builds `ClosureArm`s), so they DON'T need `match*` lowering; only a
        // clause carrying a literal/destructuring pattern — or a `:when` guard,
        // which the native path ignores (ADR-226) — does.
        return !forms.iter().all(|&f| is_arity_clause(heap, f))
            || forms.iter().any(|&f| clause_has_when_guard(heap, f));
    }
    // single-clause: a guard, a pattern optional or rest, or a pattern in a required slot?
    let params = match heap.list_or_vector_items(forms[0]) {
        Some(p) => p,
        None => return false,
    };
    let body = &forms[1..];
    let body = match body.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if body.len() > 1 => &body[1..],
        _ => body,
    };
    if single_clause_needs_match(heap, &params) || single_clause_guard(body).is_some() {
        return true;
    }
    let required_end = params
        .iter()
        .position(|&p| matches!(p.unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP)))
        .unwrap_or(params.len());
    params[..required_end].iter().any(|&p| !is_sym(p))
}

/// Lower a `fn` that is multi-clause, or single-clause with pattern(s) in its
/// required parameters, into a plain `fn` plus the Brood `match*` engine.
/// `Ok(None)` for an ordinary single-clause `fn` (left as-is); an error for a
/// **vector** where a parameter list belongs (ADR-010, item 1 of the syntax
/// finalisation) — rejected rather than misread.
pub(super) fn lower_fn(heap: &mut Heap, items: &[Value]) -> Result<Option<Value>, LispError> {
    let forms = &items[1..];

    // Multi-clause: an optional leading docstring, then every form a clause. The
    // docstring sits *before* the clauses here (a single-clause fn's docstring
    // sits after the param list and is peeled below); keep it as the lowered
    // fn's leading body form so `make_closure` still finds it.
    {
        let (doc, clauses): (Option<Value>, &[Value]) = match forms.first().map(|v| v.unpack()) {
            Some(ValueRef::Str(_)) if forms.len() > 1 => (Some(forms[0]), &forms[1..]),
            _ => (None, forms),
        };
        // Clojure's vector-headed clauses — `([x] :one) ([x y] :two)`. Caught
        // *before* the list-headed check so it can't fall through to the
        // single-clause path and be read as one long pattern parameter list.
        if has_vector_clause_head(heap, clauses) {
            return Err(crate::eval::vector_binding_container_error("fn"));
        }
        if !clauses.is_empty() && clauses.iter().all(|&f| is_clause(heap, f)) {
            // This IS a multi-clause fn — never fall through to the single-clause
            // path below (which would misread the first clause as a param list).
            if let Some(e) = shadowed_clause_error(heap, clauses) {
                return Err(e);
            }
            if clauses.iter().all(|&f| is_arity_clause(heap, f))
                && !clauses.iter().any(|&f| clause_has_when_guard(heap, f))
            {
                // Arity-only AND guard-free: dispatches natively (the evaluator's
                // `make_closure` builds one `ClosureArm` per clause, bound by argument
                // count — no rest-list, no `match*`). Leave it un-lowered. A `:when`
                // guard forces the `match*` path below, which evaluates it (ADR-226).
                return Ok(None);
            }
            // At least one literal/destructuring *pattern* clause, or a `:when` *guard*
            // clause (ADR-226) → lower the whole dispatch to the `match*` engine, where each
            // head is a list pattern over the argument list: `&` is that pattern's rest
            // (ADR-434) and `&optional` its optional elements (ADR-619), so a pattern clause
            // takes the same head grammar as an arity clause.
            let g = value::gensym("args");
            let params = heap.list(vec![value::sym(kw::AMP), g]);
            let mut mexpr = vec![value::sym(kw::MATCH_STAR), value::kw("fn"), g];
            mexpr.extend_from_slice(clauses); // fn clauses are already match* clauses
            let body = heap.list(mexpr);
            let mut lowered = vec![value::sym(kw::FN), params];
            if let Some(d) = doc {
                lowered.push(d);
            }
            lowered.push(body);
            return Ok(Some(heap.list(lowered)));
        }
    }

    // Single-clause: forms[0] is the parameter list, forms[1..] the body.
    let param_form = match forms.first() {
        Some(&f) => f,
        None => return Ok(None),
    };
    // `(fn [x y] …)` — a vector parameter list. Rejected here rather than accepted
    // as a second spelling: lists-for-code is the rule, and tolerating the vector
    // is what let `(let [[a 1] [b 2]] …)`-shaped mistakes destructure instead of
    // erroring (ADR-010).
    if matches!(param_form.unpack(), ValueRef::Vector(_)) {
        return Err(crate::eval::vector_binding_container_error("fn"));
    }
    let body = &forms[1..];
    let params = match heap.list_or_vector_items(param_form) {
        Some(p) => p,
        None => return Ok(None),
    };

    // Peel a leading docstring (a string literal with more body after it) so it
    // stays the *first* form of the lowered `fn` — otherwise `make_closure`'s
    // docstring detection misses it once the body is wrapped in the refutable
    // bind + `do`. (`(fn ([x y]) "doc" body)` would lose its doc otherwise.)
    let (doc, body) = match body.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if body.len() > 1 => (Some(body[0]), &body[1..]),
        _ => (None, body),
    };

    // A pattern in an `&optional` slot, or a pattern rest: the head is a list pattern over the
    // arguments, as a clause's is (ADR-619), so it lowers as a one-clause fn.
    if single_clause_needs_match(heap, &params) {
        let g = value::gensym("args");
        let rest_params = heap.list(vec![value::sym(kw::AMP), g]);
        let mut clause = vec![param_form];
        clause.extend_from_slice(body);
        let clause = heap.list(clause);
        let dispatch = heap.list(vec![value::sym(kw::MATCH_STAR), value::kw("fn"), g, clause]);
        let mut lowered = vec![value::sym(kw::FN), rest_params];
        if let Some(doc) = doc {
            lowered.push(doc);
        }
        lowered.push(dispatch);
        return Ok(Some(heap.list(lowered)));
    }

    // Patterns in required slots (before &optional / & rest) bind refutably, keeping the
    // native parameter list.
    let required_end = params
        .iter()
        .position(|&p| matches!(p.unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP)))
        .unwrap_or(params.len());
    let guard = single_clause_guard(body);
    if guard.is_none() && !params[..required_end].iter().any(|&p| !is_sym(p)) {
        return Ok(None); // no pattern in the required params and no guard — ordinary fn
    }

    // Replace each required pattern slot with a fresh symbol; bind it refutably.
    let mut new_params = params.clone();
    let mut binds: Vec<(Value, Value)> = Vec::new();
    for (idx, &p) in params[..required_end].iter().enumerate() {
        if !is_sym(p) {
            let g = value::gensym("arg");
            new_params[idx] = g;
            binds.push((g, p));
        }
    }
    // A guard wraps the body, inside the pattern binds so it sees their names; when it fails
    // the call raises the match error a failed clause does.
    let mut acc = match guard {
        Some((test, rest)) => {
            let then = make_do(heap, rest);
            let fail = guard_failure(heap, &new_params, param_form);
            heap.list(vec![value::sym(kw::IF), test, then, fail])
        }
        None => make_do(heap, body),
    };
    for &(g, pat) in binds.iter().rev() {
        acc = refutable_bind(heap, kw::FN, g, pat, acc);
    }
    let new_param_form = heap.list(new_params);
    let mut lowered = vec![value::sym(kw::FN), new_param_form];
    if let Some(doc) = doc {
        lowered.push(doc); // keep the docstring as the leading body form
    }
    lowered.push(acc);
    Ok(Some(heap.list(lowered)))
}
