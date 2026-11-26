//! Same-file inference: the value types of a file's undeclared globals, the signatures of
//! its own functions inferred from their forms (a bounded return fixpoint, per-clause arms
//! of a multi-clause `defn`, inline pattern-clause `fn` literals), and the joint fixpoint of
//! caller-derived parameter types and returns (ADR-341).

use super::*;

/// A surface `(defn name clause…)` / `(defn- name clause…)` whose tail is entirely
/// clauses — arity clauses `((a b) …)` or pattern clauses `(((x y & acc) "+") …)` —
/// returning the name and those clauses. `None` for a single-clause definition (which
/// the ordinary parameter inference already reads) or any other form. A clause is a list
/// whose head is a list (`is_clause`'s shape; a vector head is Clojure's spelling and is
/// rejected by the expander), which is why `(defn f ([a b]) body)` — one destructuring
/// clause followed by a body form — is not two clauses.
fn defn_clauses(heap: &Heap, form: Value) -> Option<(Symbol, Vec<Value>)> {
    let items = list_items(heap, form)?;
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    if !value::symbol_is(head, crate::core::keywords::DEFN)
        && !value::symbol_is(head, crate::core::keywords::DEFN_PRIVATE)
    {
        return None;
    }
    let Some(&Value::Sym(name)) = items.get(1) else {
        return None;
    };
    let rest = items.get(2..)?;
    // A leading docstring sits before the clauses, as in a multi-clause `fn`.
    let rest = match rest.first() {
        Some(Value::Str(_)) if rest.len() > 1 => &rest[1..],
        _ => rest,
    };
    let is_clause = |f: Value| -> bool {
        list_items(heap, f)
            .is_some_and(|items| matches!(items.first(), Some(Value::Pair(_)) | Some(Value::Nil)))
    };
    if rest.len() < 2 || !rest.iter().all(|&f| is_clause(f)) {
        return None;
    }
    Some((name, rest.to_vec()))
}

/// Walk `form` (un-expanded) for inline pattern-clause `fn` literals and record each
/// one's `(heads, tail)` arms in `ctx` under its printed clause-head-list key — the side
/// channel [`sigs::clause_lambda_ret`] reads back at call sites, where the expanded tree
/// holds only the lowered variadic `%match` lambda (see `Ctx::fn_literal_clauses`).
/// Quoted/quasiquoted subtrees are data/templates and `comment` never runs — skipped,
/// like the other surface passes.
fn record_fn_literal_clauses(heap: &Heap, form: Value, ctx: &mut Ctx) {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
            Value::Pair(_) => {
                let Some(items) = list_items(heap, v) else {
                    continue;
                };
                if let Some(&Value::Sym(head)) = items.first() {
                    if value::symbol_is(head, kw::QUOTE)
                        || value::symbol_is(head, kw::QUASIQUOTE)
                        || value::symbol_is(head, kw::COMMENT)
                    {
                        continue;
                    }
                    if walk::is_fn_head(head)
                        && let Some((key, arms, printed)) = pattern_fn_literal(heap, &items)
                    {
                        ctx.add_fn_literal_clauses(key, arms, printed);
                    }
                }
                work.extend(items);
            }
            _ => {}
        }
    }
}

/// Read `items` (a surface `(fn …)` form's items) as a literal the expander will lower
/// to `%match` — clause style with at least one pattern head or guard, or the plain
/// single-clause style with a destructuring parameter. `None` for anything the expander
/// keeps as a native `fn` (plain params, arity-only clauses), which the direct path in
/// `clause_lambda_ret` / `lambda_ret` already types. Returns the head-list key (printed
/// exactly as the lowering's `(quote heads)` datum prints), the `(heads, tail)` arms,
/// and the full printed clause list for the registry's collision test.
#[allow(clippy::type_complexity)]
fn pattern_fn_literal(
    heap: &Heap,
    items: &[Value],
) -> Option<(String, Vec<(Vec<Value>, Value)>, String)> {
    let forms = match items.get(1..) {
        Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
        Some(rest) => rest,
        None => return None,
    };
    if forms.is_empty() {
        return None;
    }
    let is_pattern_head = |h: &Value| !matches!(h, Value::Sym(_));
    let print = |v: Value| crate::syntax::printer::print(heap, v);
    // Clause style: every form is `(head-list body…)`.
    let as_clauses: Option<Vec<_>> = forms.iter().map(|&c| sigs::clause_arm(heap, c)).collect();
    if let Some(arms) = as_clauses {
        let lowered = arms
            .iter()
            .any(|a| a.guard.is_some() || a.heads.iter().any(is_pattern_head));
        if !lowered {
            return None; // arity-only clauses stay native `fn` clauses
        }
        let head_lists: Vec<String> = forms
            .iter()
            .map(|&c| Some(print(*list_items(heap, c)?.first()?)))
            .collect::<Option<_>>()?;
        let key = format!("({})", head_lists.join(" "));
        let printed: String = forms
            .iter()
            .map(|&c| print(c))
            .collect::<Vec<_>>()
            .join(" ");
        let out = arms
            .into_iter()
            .map(|a| a.body.last().map(|&t| (a.heads, t)))
            .collect::<Option<Vec<_>>>()?;
        return Some((key, out, printed));
    }
    // Plain single-clause style with a destructuring parameter: `(fn ([k v]) body…)`.
    let plist = *forms.first()?;
    let heads = match plist {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, plist)?,
        _ => return None,
    };
    if !heads.iter().any(is_pattern_head) {
        return None; // plain params — native `fn`, `lambda_ret`'s case
    }
    let tail = *forms.get(1..)?.last()?;
    let key = format!("({})", print(plist));
    let printed = format!("{} {}", print(plist), print(tail));
    Some((key, vec![(heads, tail)], printed))
}

/// The most rounds the joint fixpoint of caller-derived parameters and returns runs
/// before it gives up (and says so).
const MAX_JOINT_ROUNDS: usize = 32;

/// The most rounds the same-file return inference runs before the names still moving
/// fall back to an `any` return (and the file says so).
const MAX_RETURN_ROUNDS: usize = 16;

/// The value type of each *undeclared* global defined exactly once by `(def g
/// <non-fn-expr>)`: the RHS's `expr_ty` becomes `g`'s current-image type, read as
/// `dynamic_within` (the `∩` relation — reload-safe, warns only on provable
/// disjointness). Skipped: a global defined more than once, a macro, a dynamic or
/// earmuffed variable (rebound over its lifetime), and a function value (its arrow is
/// inferred separately).
///
/// Reads `top_level_defs`, not `expanded`, so the `def` inside a `def-`'s `(do (def g …)
/// (%mark-private 'g))` is seen.
pub(super) fn infer_global_value_types(heap: &Heap, expanded: &[Value], ctx: &mut Ctx) {
    let def_count = def_counts(heap, expanded);
    for form in top_level_defs(heap, expanded) {
        let Some((name, rhs)) = def_name_and_value(heap, form) else {
            continue;
        };
        // An earmuffed `*name*` is dynamic by convention (the lazy-init `(when (nil? *g*)
        // (def *g* …))` pattern), so pinning it to its load-time value would false-positive
        // once reassigned — and flow into the return of every function that reads it.
        if value::is_gensym(name)
            || def_count.get(&name) != Some(&1)
            || ctx.is_file_macro(name)
            || heap.is_dynamic(name)
            || infer::is_earmuffed(name)
        {
            continue;
        }
        if let Some(ty) = infer::expr_ty(heap, rhs, ctx)
            && !ty.contains_tag(value::Tag::Fn)
            && !ty.contains_tag(value::Tag::Native)
        {
            ctx.add_inferred_value_ty(name, ty);
        }
    }
}

/// How many times each name is `def`'d anywhere in the expanded tree.
fn def_counts(heap: &Heap, expanded: &[Value]) -> HashMap<Symbol, usize> {
    let mut counts = HashMap::new();
    for &form in expanded {
        count_defs(heap, form, &mut counts);
    }
    counts
}

/// The functions whose signatures the file infers: every top-level `(def name rhs)`
/// defined once, undeclared, and not a macro, dynamic variable or gensym.
fn inference_candidates(heap: &Heap, expanded: &[Value], ctx: &Ctx) -> Vec<(Symbol, Value)> {
    let def_count = def_counts(heap, expanded);
    let found: Vec<(Symbol, Value)> = top_level_defs(heap, expanded)
        .into_iter()
        .filter_map(|form| {
            let (name, rhs) = def_name_and_value(heap, form)?;
            (def_count.get(&name) == Some(&1)
                && !ctx.is_file_macro(name)
                && !heap.is_dynamic(name)
                && !value::is_gensym(name)
                && (ctx.declared_sig(name).is_none() || ctx.is_open_ret(name)))
            .then_some((name, rhs))
        })
        .collect();
    callees_first(heap, found)
}

/// `candidates` ordered so each function comes after the same-file functions it calls
/// (a depth-first post-order of the call graph; a cycle keeps its definition order). The
/// return fixpoint walks this list once per round, and a callee typed earlier in the same
/// round is what its caller reads — so callee-first, a chain of any length settles in one
/// round. In definition order it settled one level per round, and the order the file
/// happened to define a chain in decided whether it settled inside the round cap: the same
/// 41-function chain reported its finding written one way and went silent written the other.
fn callees_first(heap: &Heap, candidates: Vec<(Symbol, Value)>) -> Vec<(Symbol, Value)> {
    let index: HashMap<Symbol, usize> = candidates
        .iter()
        .enumerate()
        .map(|(i, &(name, _))| (name, i))
        .collect();
    // The same-file candidates each body mentions, in the order it mentions them.
    let calls: Vec<Vec<usize>> = candidates
        .iter()
        .map(|&(_, rhs)| {
            let mut seen = Vec::new();
            let mut stack = vec![rhs];
            while let Some(form) = stack.pop() {
                match form {
                    Value::Sym(sym) => {
                        if let Some(&callee) = index.get(&sym)
                            && !seen.contains(&callee)
                        {
                            seen.push(callee);
                        }
                    }
                    Value::Pair(_) | Value::Vector(_) => {
                        if let Some(items) = heap.list_or_vector_items(form) {
                            stack.extend(items.into_iter().rev());
                        }
                    }
                    _ => {}
                }
            }
            seen
        })
        .collect();
    // Iterative post-order: a node is emitted once every callee below it has been.
    let mut placed = vec![false; candidates.len()];
    let mut on_path = vec![false; candidates.len()];
    let mut order = Vec::with_capacity(candidates.len());
    for root in 0..candidates.len() {
        if placed[root] {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        on_path[root] = true;
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            if let Some(&callee) = calls[node].get(*next) {
                *next += 1;
                if !placed[callee] && !on_path[callee] {
                    on_path[callee] = true;
                    stack.push((callee, 0));
                }
            } else {
                stack.pop();
                on_path[node] = false;
                placed[node] = true;
                order.push(node);
            }
        }
    }
    order.into_iter().map(|i| candidates[i]).collect()
}

/// **Same-file function inference.** The file being checked isn't loaded, so `sig_of`'s
/// loaded-closure inference can't see its own `(defn …)`s. Infer each candidate's return
/// from its FORM (`sigs::infer_return_from_form`) and record it, with its inferred
/// parameter demands (ADR-190, which under-constrain, so a flagged argument genuinely
/// errors), so a same-file call flows the result as a loaded-function call does.
///
/// A bounded fixpoint resolves callees leaf-up: `infer_return_from_form` defers until
/// every function its return depends on is recorded. Runs after the global value types,
/// which a body may read. Returns the candidates, which the caller-derived parameter
/// pass derives next.
pub(super) fn infer_same_file_functions(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ctx: &mut Ctx,
    file_ns_name: Option<&str>,
    out: &mut Vec<Finding>,
) -> Vec<(Symbol, Value)> {
    let derive_dbg = derive_debug();
    let candidates = inference_candidates(heap, expanded, ctx);
    // Each candidate's form stays reachable for call-site specialization
    // (`sigs::specialized_ret`): a call whose flat inferred return is `any` re-types the
    // body under the call's own argument types.
    for &(name, rhs) in &candidates {
        ctx.add_fn_form(name, rhs);
    }
    // A multi-clause `defn`'s clauses — read from the UN-EXPANDED form, the only place a
    // pattern- or `:when`-dispatched definition still has clauses (ADR-226) — go in BEFORE
    // the fixpoint: a same-file caller typed inside it needs the arms. The per-arm
    // signatures are derived again after the fixpoint, when every callee's return is final.
    let mut clause_defs: Vec<(Symbol, Vec<Value>)> = Vec::new();
    for &form in forms {
        if let Some((name, clauses)) = defn_clauses(heap, form) {
            let qualified = qualify_decl_name(ctx, file_ns_name, name);
            if let Some(arms) = sigs::infer_overload_from_clauses(heap, &clauses, ctx) {
                ctx.add_inferred_overload(qualified, arms);
            }
            ctx.add_clause_arms(qualified, clauses.clone());
            clause_defs.push((qualified, clauses));
        }
    }
    // Inline PATTERN-clause `fn` literals get the same surface record, keyed by their
    // printed clause-head list: the expanded tree holds only the lowered `%match` lambda,
    // whose embedded `(%match-error … (quote heads))` call is how
    // `sigs::clause_lambda_ret` finds its way back here from a combinator call site.
    for &form in forms {
        record_fn_literal_clauses(heap, form, ctx);
        lint_unused_pattern_binders(heap, form, out);
    }
    // The names still moving when the bound ends the ascent: a value on an ascending chain
    // cut short is an UNDER-approximation, so each falls back to its demands with an `any`
    // return (sound), and the file says so.
    let mut still_moving: Vec<Symbol> = Vec::new();
    let mut older_returns: HashMap<Symbol, Ty> = HashMap::new();
    for round in 0..MAX_RETURN_ROUNDS {
        still_moving.clear();
        for &(name, rhs) in &candidates {
            let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), ctx) else {
                continue;
            };
            let current = ctx.inferred_fn_sig(name).map(|s| s.ret);
            let ret = ascend_return(ret, current.as_ref(), older_returns.get(&name), round);
            if let Some(current) = current.clone() {
                older_returns.insert(name, current);
            }
            if current != Some(ret.clone()) {
                // Parameters are body-derived (independent of this fixpoint, which only
                // resolves returns), so recomputing them is stable.
                let sig = match sigs::infer_params_from_form(heap, rhs, ctx) {
                    Some(demands) => demands.into_sig(ret),
                    None => crate::types::Sig::new(Vec::new(), ret),
                };
                if derive_dbg {
                    eprintln!(
                        "[derive] same-file return {} -> {}",
                        value::symbol_name_ref(name),
                        sig.ret
                    );
                }
                ctx.add_inferred_fn_sig(name, sig);
                still_moving.push(name);
            }
        }
        if still_moving.is_empty() {
            break;
        }
    }
    for &name in &still_moving {
        let rhs = candidates
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|&(_, rhs)| rhs);
        let sig = rhs
            .and_then(|rhs| sigs::infer_params_from_form(heap, rhs, ctx))
            .map(|demands| demands.into_sig(Ty::ANY))
            .unwrap_or_else(|| crate::types::Sig::new(Vec::new(), Ty::ANY));
        ctx.add_inferred_fn_sig(name, sig);
        out.push(Finding::new(
            Lint::GaveUp,
            rhs.and_then(|rhs| heap.form_pos_only(rhs)),
            format!(
                "checker gave up: the return of {} was still moving after {MAX_RETURN_ROUNDS} inference rounds — read as unknown here and by every caller",
                value::symbol_name_ref(name)
            ),
        ));
    }
    // A **multi-arm** candidate has no single signature, so record each arm's own; the
    // call check rules a call out only when every arity-relevant arm rejects it.
    for &(name, rhs) in &candidates {
        if let Some(arms) = sigs::infer_overload_from_form(heap, rhs, ctx) {
            ctx.add_inferred_overload(name, arms);
        }
    }
    // …and from the un-expanded `defn` clauses, now that every same-file return an arm's
    // tail may name has settled.
    for (qualified, clauses) in &clause_defs {
        if let Some(arms) = sigs::infer_overload_from_clauses(heap, clauses, ctx) {
            ctx.add_inferred_overload(*qualified, arms);
        }
    }
    // A candidate whose return stayed deferred (e.g. it returns an ability op) still gets
    // its parameter demands with an `any` return, so its callers are argument-checked.
    for &(name, rhs) in &candidates {
        if ctx.inferred_fn_sig(name).is_none()
            && let Some(demands) = sigs::infer_params_from_form(heap, rhs, ctx)
        {
            ctx.add_inferred_fn_sig(name, demands.into_sig(Ty::ANY));
        }
    }
    settle_guards(heap, &candidates, ctx);
    candidates
}

/// Bound on [`settle_guards`]: one round per level of a predicate built from predicates.
const MAX_GUARD_ROUNDS: usize = 4;

/// Re-read each candidate's inferred guard (ADR-725) until none moves. The return fixpoint
/// above re-records a signature only when its RETURN changes, and a predicate's return is
/// `bool` from the first round — so `(defn a? (x) (b? x))` above a later `b?` kept the guard
/// it read while `b?` had none. A guard only ever grows here (a callee's guard appearing),
/// and the bound cuts a pathological chain at a weaker, still-true guard.
fn settle_guards(heap: &Heap, candidates: &[(Symbol, Value)], ctx: &mut Ctx) {
    for _ in 0..MAX_GUARD_ROUNDS {
        let mut changed = false;
        for &(name, rhs) in candidates {
            let Some(current) = ctx.inferred_fn_sig(name) else {
                continue;
            };
            // Only a predicate carries a guard (`ParamDemands::into_sig`): a function whose
            // return is not a boolean, and has no guard to lose, cannot move.
            if !sigs::is_predicate_return(&current.ret)
                && current.guard.is_none()
                && current.guard_else.is_none()
            {
                continue;
            }
            let Some(demands) = sigs::infer_params_from_form(heap, rhs, ctx) else {
                continue;
            };
            let next = demands.into_sig(current.ret.clone());
            if (next.guard.as_ref(), next.guard_else.as_ref())
                != (current.guard.as_ref(), current.guard_else.as_ref())
            {
                let mut sig = current;
                sig.guard = next.guard;
                sig.guard_else = next.guard_else;
                ctx.add_inferred_fn_sig(name, sig);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// One step of a return's ascent (ADR-349/350), shared by both return fixpoints: an
/// interval that moved since the last round goes to its infinity, a value that nests a
/// previous one folds into a recursive type, and past the early rounds the rest is cut to
/// a depth — so `(cons x (self …))`, ⊥ in its first round, cannot lengthen forever.
fn ascend_return(mut ret: Ty, current: Option<&Ty>, older: Option<&Ty>, round: usize) -> Ty {
    if let Some(previous) = current {
        ret = ret.widen_intervals_against(previous);
    }
    for previous in current.into_iter().chain(older) {
        if let Some(folded) = Ty::fold_recursive(previous, &ret) {
            ret = folded;
            break;
        }
    }
    if round >= sigs::WIDEN_AFTER_ROUND {
        ret = ret.widened_below(sigs::WIDEN_DEPTH);
    }
    ret
}

/// **Caller-derived parameter types** (ADR-341). The union of what a function's call sites
/// in this file hand its parameters is a sound binding for the walk of its body, and for
/// the return its same-file callers read — where same-file inference bound the parameters
/// to their bottom-up DEMANDS alone (`(+ i 1)` says `number`; the callers say `int`). This
/// is what lets a leaf `sig` be enough: `int` flows from the public entry down through
/// every helper without a declaration on each.
///
/// Every candidate is derived, public or private: a derived type is a fact about THIS
/// FILE's calls, so a body warning under it says "every call in this file would fail here",
/// true whatever callers exist elsewhere; a caller in another file reads the demand-based
/// loaded inference.
///
/// The derivation and the returns feed each other (a helper's sharper return is a caller's
/// sharper argument), so this is a JOINT least fixpoint. Starting from the demand-based
/// returns, two helpers that hand each other an index through their returns could never
/// leave `number`, so the returns START at ⊥ and rise with the parameters (Kleene), and only
/// the fixpoint is used. A round that cannot type a return keeps the demand-based one. No
/// fixpoint within [`MAX_JOINT_ROUNDS`]: everything is restored as it was, and the file
/// says so.
pub(super) fn derive_caller_params(
    heap: &Heap,
    expanded: &[Value],
    all_candidates: &[(Symbol, Value)],
    ctx: &mut Ctx,
    out: &mut Vec<Finding>,
    timing: &mut DeriveTiming,
) {
    let candidates: HashMap<Symbol, Value> = all_candidates.iter().copied().collect();
    if candidates.is_empty() {
        return;
    }
    let original: HashMap<Symbol, Option<Sig>> = candidates
        .keys()
        .map(|&name| (name, ctx.inferred_fn_sig(name)))
        .collect();
    // Only a LIVE function's return starts at ⊥. An escaped or site-less one is never
    // derived, and its in-file callers read its return while the others iterate: a ⊥ there
    // would make THEIR derived parameters under-approximate.
    let live = sigs::live_private_functions(heap, expanded, &candidates, ctx).unwrap_or_default();
    // The count relations between a live function's parameters that its sites establish
    // (ADR-350) — syntactic, so settled once here, for every round and for the walk.
    sigs::derive_count_aliases(heap, expanded, &candidates, &live, ctx);
    for (&name, sig) in &original {
        if let (Some(sig), true) = (sig, live.contains_key(&name)) {
            let mut floor = sig.clone();
            floor.ret = Ty::NEVER;
            ctx.add_inferred_fn_sig(name, floor);
        }
    }
    // A live function whose return never typed and is still at the ⊥ floor.
    let at_floor = |ctx: &Ctx, typed: &HashSet<Symbol>, name: Symbol, sig: &Sig| {
        !typed.contains(&name)
            && ctx.inferred_fn_sig(name).is_some_and(|s| s.ret.is_never())
            && !sig.ret.is_never()
    };
    let mut previous: HashMap<Symbol, Vec<Option<Ty>>> = HashMap::new();
    // Which returns a round has TYPED — as opposed to typed as `never`, which is a real
    // answer (every call raises) and must not be mistaken for a floor that never typed.
    let mut typed: HashSet<Symbol> = HashSet::new();
    let mut converged = false;
    // Whether anything the derivation reads moved since it last ran. When nothing did,
    // re-running it would reproduce `previous` exactly (it is a deterministic function of
    // the returns and its seed), so the round reuses it.
    let mut inputs_moved = true;
    // Each return's value from the round before last, for the fold (a value that recurses
    // through another function grows every other round).
    let mut older_returns: HashMap<Symbol, Ty> = HashMap::new();
    // Every value each return has held, to tell a cycle from a move (see the loop).
    let mut returns_held: HashMap<Symbol, Vec<Ty>> = HashMap::new();
    // The site walks, memoised per form across every round, with the names whose return or
    // derived parameters moved since the last derivation — `None` walks everything.
    let mut site_cache = sigs::SiteCache::new(heap, expanded, &candidates);
    let mut moved: Option<HashSet<Symbol>> = None;
    let mut derivation_declined = false;
    for round in 0..MAX_JOINT_ROUNDS {
        let round_started = std::time::Instant::now();
        let derivation_ran = inputs_moved;
        // A specialization is typed under the round's returns and derived parameters; the
        // memo of them must not outlive the round.
        sigs::clear_specializations();
        let derived = if inputs_moved {
            sigs::caller_derived_params(
                heap,
                expanded,
                &candidates,
                &live,
                ctx,
                &previous,
                &mut site_cache,
                moved.as_ref(),
            )
        } else {
            previous.clone()
        };
        timing.deriving += round_started.elapsed();
        timing.rounds += 1;
        // A derivation that declined leaves every parameter unknown for the rest of this
        // file: say so, once.
        if let Some(why) = sigs::take_derivation_decline()
            && !derivation_declined
        {
            derivation_declined = true;
            out.push(Finding::new(
                    Lint::GaveUp,
                    None,
                    match why {
                        sigs::DeriveDecline::Opaque => "checker gave up: an unexpanded macro call keeps every caller-derived parameter unknown in this file".to_string(),
                        sigs::DeriveDecline::NoFixpoint => format!(
                            "checker gave up: the caller-derived parameters did not settle in {} rounds — every parameter is read as unknown in this file",
                            sigs::MAX_DERIVE_ROUNDS
                        ),
                    },
                ));
        }
        let mut moved_now: HashSet<Symbol> = derived
            .keys()
            .chain(previous.keys())
            .copied()
            .filter(|name| derived.get(name) != previous.get(name))
            .collect();
        ctx.set_derived_params(derived.clone());
        let mut returns_moved = false;
        // In DEFINITION order, never the map's: a return re-read here is applied at once,
        // so a later name reads an earlier one's new value and the widening is
        // history-dependent — hash order made the verdict nondeterministic (KI-158).
        for &(name, rhs) in all_candidates {
            let Some(Some(base)) = original.get(&name) else {
                continue;
            };
            // Only a DERIVED candidate's return is re-read: an escaped or site-less one
            // keeps its demand-based one, whose `any` lets the recursive call-site
            // specialization sharpen it.
            if ctx.derived_params(name).is_none() {
                continue;
            }
            // A return this round cannot type stays where it is (⊥, or the last round's):
            // the demand-based return would put a value ABOVE the least fixpoint into a
            // monotone ascent, and nothing could bring it back down.
            let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), ctx) else {
                continue;
            };
            typed.insert(name);
            let current = ctx.inferred_fn_sig(name).map(|s| s.ret);
            let ret = ascend_return(ret, current.as_ref(), older_returns.get(&name), round);
            // A return may move either way between rounds (a parameter read as `any` one
            // round is derived the next, and the return sharpens), so nothing here is a
            // monotone ascent, and a re-typing can CYCLE: one came back as `vector`, then
            // `vector<(tuple any)>`, then `vector` again, for good, and the fixpoint it never
            // reached left every parameter of the file unknown (KI-416, b3d-mass). A return
            // that goes back to a value it has already held is in such a cycle: it joins the
            // current one instead (sound: the function may give either), which ends it.
            let ret = match &current {
                Some(held)
                    if ret != *held
                        && returns_held
                            .get(&name)
                            .is_some_and(|before| before.contains(&ret)) =>
                {
                    ret.union(held.clone())
                }
                _ => ret,
            };
            if let Some(current) = current.clone() {
                returns_held.entry(name).or_default().push(current.clone());
                older_returns.insert(name, current);
            }
            if current != Some(ret.clone()) {
                let mut sig = base.clone();
                sig.ret = ret;
                ctx.add_inferred_fn_sig(name, sig);
                returns_moved = true;
                moved_now.insert(name);
            }
        }
        if derived == previous && !returns_moved {
            // A live function whose return never typed is still at the floor, and the
            // others have been reading it as ⊥. Lift it to its demand-based answer (sound,
            // above ⊥, so the ascent stays monotone) and go again; only a round with
            // nothing left to lift is the fixpoint.
            let mut lifted = false;
            for (&name, sig) in &original {
                if let Some(sig) = sig
                    && at_floor(ctx, &typed, name, sig)
                {
                    ctx.add_inferred_fn_sig(name, sig.clone());
                    lifted = true;
                    moved_now.insert(name);
                }
            }
            if !lifted {
                converged = true;
                break;
            }
            inputs_moved = true;
        } else {
            inputs_moved = returns_moved;
        }
        // What the next derivation must re-walk: everything that moved since the cache's
        // last walk — this round's movers, plus the earlier ones when this round skipped
        // the derivation.
        moved = Some(match moved.take() {
            Some(mut earlier) if !derivation_ran => {
                earlier.extend(moved_now);
                earlier
            }
            _ => moved_now,
        });
        previous = derived;
    }
    timing.site_walks = site_cache.walks;
    timing.site_reuses = site_cache.reuses;
    if !converged {
        for (&name, sig) in &original {
            if let Some(sig) = sig {
                ctx.add_inferred_fn_sig(name, sig.clone());
            }
        }
        ctx.set_derived_params(HashMap::new());
        // Sound — every return is back at its demand-based one and no parameter is derived
        // — and reported: otherwise a file whose derivation gave up reads as clean.
        out.push(Finding::new(
            Lint::GaveUp,
            None,
            format!("checker gave up: the caller-derived parameters and returns did not settle in {MAX_JOINT_ROUNDS} joint rounds — every parameter is read as unknown in this file"),
        ));
        return;
    }
    // A candidate that was not derived, or whose return never typed (still at the floor):
    // give it its demand-based answer back.
    for (&name, sig) in &original {
        if let Some(sig) = sig
            && (ctx.derived_params(name).is_none() || at_floor(ctx, &typed, name, sig))
        {
            ctx.add_inferred_fn_sig(name, sig.clone());
        }
    }
    // …and the candidates that were NOT derived (no site, or an escape), whose returns were
    // read before any return was sharpened, are read again over the sharper ones.
    let undecided: Vec<(Symbol, Value)> = all_candidates
        .iter()
        .filter(|(name, _)| ctx.derived_params(*name).is_none())
        .copied()
        .collect();
    refresh_returns(heap, &undecided, ctx);
}

/// The same-file return fixpoint over `candidates`, run again: each function's return is
/// re-read from its form, leaf-up, until a pass records nothing new (a cap bounds a
/// pathological chain — the tail stays as it was, which is sound). The caller-derived
/// parameter pass calls it for the candidates it did not derive, once the returns beneath
/// them have been sharpened.
fn refresh_returns(heap: &Heap, candidates: &[(Symbol, Value)], ctx: &mut Ctx) {
    for _ in 0..MAX_RETURN_ROUNDS {
        let mut changed = false;
        for &(name, rhs) in candidates {
            if let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), ctx)
                && ctx.inferred_fn_sig(name).map(|s| s.ret) != Some(ret.clone())
            {
                let sig = match sigs::infer_params_from_form(heap, rhs, ctx) {
                    Some(demands) => demands.into_sig(ret),
                    None => crate::types::Sig::new(Vec::new(), ret),
                };
                ctx.add_inferred_fn_sig(name, sig);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}
