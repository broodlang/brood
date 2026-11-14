//! Call-site specialization: re-typing a function's body under the argument types of one
//! call ([`specialized_ret`]), with the re-entry guard, the fuel budget and the memo that
//! keep it bounded. The per-thread state lives in the parent, which resets it per file.

use super::*;

/// How many specializations may be live at once on this thread. Each level is one
/// `expr_ty` walk of a body; the walk has its own depth guard, so this only bounds the
/// chain of *functions* re-typed inside one another.
const MAX_SPECIALIZE_DEPTH: usize = 2;

/// [`SPECIAL_FUEL`]'s per-file budget, in arm re-typings. A normal file spends a few
/// hundred; the pathological shape above spent it in under a second.
pub(in crate::types::check) const MAX_SPECIAL_FUEL: u32 = 20_000;

/// Would [`specialized_ret`] even consider `sym` right now — fuel left, `sym` not in
/// flight, depth under the cap? The call site asks this BEFORE typing the call's
/// arguments, which is the re-walk a refusal would otherwise have paid for nothing.
fn specialization_open(sym: Symbol) -> bool {
    SPECIAL_FUEL.with(|f| f.get() > 0)
        && SPECIALIZING.with(|s| {
            let s = s.borrow();
            s.len() < MAX_SPECIALIZE_DEPTH && !s.contains(&sym)
        })
}

/// [`SPEC_ARG_NEST`]'s bound: `(f (g x))` specializes both, `(f (g (h x)))` leaves `h`
/// flat.
const MAX_SPEC_ARG_NEST: u8 = 1;

/// [`specialized_ret`] for a call form's operands: types `args` only once specialization
/// is open for `sym` (see [`specialization_open`]), so a refused question costs nothing,
/// and only [`MAX_SPEC_ARG_NEST`] deep in nested operand typing.
pub(in crate::types::check) fn specialize_call(
    heap: &Heap,
    sym: Symbol,
    args: &[Value],
    ctx: &Ctx,
) -> Option<Ty> {
    if SPEC_ARG_NEST.with(|n| n.get()) > MAX_SPEC_ARG_NEST
        || !specialization_open(sym)
        || NO_ARMS.with(|m| m.borrow().contains(&sym))
    {
        return None;
    }
    SPEC_ARG_NEST.with(|n| n.set(n.get() + 1));
    let inputs: Vec<Option<Ty>> = args
        .iter()
        .map(|&a| expr_ty(heap, a, ctx).or_else(|| global_fn_arrow(heap, a, ctx)))
        .collect();
    SPEC_ARG_NEST.with(|n| n.set(n.get() - 1));
    specialized_ret(heap, sym, &inputs, ctx)
}

/// The memo's size cap per file. A `reduce` site asks twice with two accumulator types,
/// so a hot combinator over many callees can grow this; past the cap new tuples are
/// computed but not remembered.
const MAX_SPECIAL_MEMO: usize = 4096;

struct SpecializeGuard(Symbol);

impl SpecializeGuard {
    fn enter(sym: Symbol) -> Option<SpecializeGuard> {
        let entered = SPECIALIZING.with(|s| {
            let mut set = s.borrow_mut();
            if set.len() >= MAX_SPECIALIZE_DEPTH {
                return false;
            }
            set.insert(sym)
        });
        // Lazily, so a refusal never builds — and drops — a guard for a name still in
        // flight (KI-87's shape).
        entered.then(|| SpecializeGuard(sym))
    }
}

impl Drop for SpecializeGuard {
    fn drop(&mut self) {
        SPECIALIZING.with(|s| {
            s.borrow_mut().remove(&self.0);
        });
    }
}

/// One fixed-arity arm of a function, ready to re-type: its parameter names and the tail
/// form whose type is the arm's result.
pub(super) struct FixedArm {
    /// One head per position — a parameter symbol, or (for a clause arm) a pattern.
    pub(super) heads: Vec<Value>,
    tail: Value,
}

/// The fixed-arity arms of `sym`'s definition — same-file form first (the file is not
/// loaded, and its heap binding, if any, is the OLD one), else the loaded closure. An arm
/// with `&optional` or a rest binder is left out: its frame fill differs per call, so
/// binding parameters positionally to argument types would be wrong.
/// `(arms, stable)` — `stable` when the arms came from the LOADED image, whose bodies and
/// signatures do not move during a file check, so a `None` answer for them can be
/// memoized; a same-file body's answer depends on the same-file inference fixpoint and cannot be.
fn fixed_arms_of(heap: &Heap, sym: Symbol, ctx: &Ctx) -> Option<(Vec<FixedArm>, bool)> {
    // A same-file multi-clause `defn`: its expanded form is one variadic `fn` over
    // `%match` and has no fixed arms; the un-expanded clauses are the arms.
    if let Some(clauses) = ctx.clause_arms(sym) {
        let mut out = Vec::with_capacity(clauses.len());
        for &clause in clauses {
            let arm = clause_arm(heap, clause)?;
            if arm.heads.iter().any(|&h| {
                matches!(h, Value::Sym(s)
                    if value::symbol_is(s, kw::AMP)
                        || value::symbol_is(s, kw::AMP_OPTIONAL)
                        || value::symbol_is(s, kw::AMP_REST))
            }) {
                continue;
            }
            let Some(&tail) = arm.body.last() else {
                continue;
            };
            out.push(FixedArm {
                heads: arm.heads,
                tail,
            });
        }
        return Some((out, false));
    }
    if let Some(form) = ctx.fn_form(sym) {
        return fixed_arms_of_form(heap, form).map(|arms| (arms, false));
    }
    if ctx.is_file_global(sym) {
        return None;
    }
    let Value::Fn(cid) = super::deps::obs_function(heap, sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    let mut out = Vec::with_capacity(closure.arms.len());
    for arm in closure.arms.iter() {
        if !arm.optionals.is_empty() || arm.rest.is_some() {
            continue;
        }
        let Some(&tail) = arm.body.last() else {
            continue;
        };
        out.push(FixedArm {
            heads: arm.params.iter().map(|&p| Value::Sym(p)).collect(),
            tail,
        });
    }
    Some((out, true))
}

/// Does `form` mention `sym` anywhere — as a call head or an operand, at any depth?
/// Conservative on purpose: a function whose body names itself is treated as recursive.
pub(super) fn form_mentions(heap: &Heap, form: Value, sym: Symbol) -> bool {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) if s == sym => return true,
            Value::Pair(_) => {
                if let Some(items) = list_items(heap, v) {
                    work.extend(items);
                }
            }
            Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
            _ => {}
        }
    }
    false
}

/// [`fixed_arms_of`] for a `(fn …)` form: a single-clause fn, or each clause of a
/// multi-arity one. A parameter list with a `&`/`&optional` marker or a destructuring
/// pattern is skipped.
pub(super) fn fixed_arms_of_form(heap: &Heap, fn_form: Value) -> Option<Vec<FixedArm>> {
    let items = list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    let plain_params = |plist: Value| -> Option<Vec<Value>> {
        let raw = match plist {
            Value::Vector(id) => heap.vector(id).to_vec(),
            Value::Nil | Value::Pair(_) => list_items(heap, plist)?,
            _ => return None,
        };
        for p in &raw {
            match p {
                Value::Sym(s) if !value::symbol_name_ref(*s).starts_with('&') => {}
                _ => return None,
            }
        }
        Some(raw)
    };
    let mut arms = Vec::new();
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        let clauses = match items.get(1..) {
            Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
            Some(rest) => rest,
            None => return None,
        };
        for &clause in clauses {
            let citems = list_items(heap, clause)?;
            if citems.len() < 2 {
                continue;
            }
            let Some(params) = plain_params(*citems.first()?) else {
                continue;
            };
            arms.push(FixedArm {
                heads: params,
                tail: *citems.last()?,
            });
        }
    } else {
        let plist = *items.get(1)?;
        let body_start = match (items.get(2), items.get(3)) {
            (Some(Value::Str(_)), Some(_)) => 3,
            _ => 2,
        };
        let tail = *items.get(body_start..).and_then(|b| b.last())?;
        if let Some(params) = plain_params(plist) {
            arms.push(FixedArm {
                heads: params,
                tail,
            });
        }
    }
    Some(arms)
}

/// **Call-site specialization**: the return type of `sym` when called with arguments of
/// types `inputs` (`None` = unknown), computed by re-typing its body with each parameter
/// bound to the corresponding argument type — what [`infer::lambda_ret`] already does for
/// an inline `(fn …)`, extended to a named function. Closes the gap where a function whose
/// parameter nothing in its own body constrains reads as `(any -> any)`: `(defn wrap (x)
/// … x)` called with an int IS an int, and `(defn step (acc v) (conj acc (f v)))` handed
/// an empty-list accumulator by `reduce` builds a list. Reach for it only after the flat
/// answer (a declaration, an inferred sig) has come back `any` or nothing.
///
/// Sound for the reason `lambda_ret` is: this is FORWARD result typing under the call's
/// actual argument types, never a check of the body, so it opens no false-positive
/// class; and a self-recursive branch is skipped via `with_inferring_self`, so a
/// tail-recursive function types from its base cases. Multi-arm: the union over every
/// fixed-arity arm of the call's arity, an over-approximation. `None` when nothing can be
/// learned — no argument type known, no fixed arm of that arity, a body that cannot be
/// typed, a cycle, or the depth cap.
pub(in crate::types::check) fn specialized_ret(
    heap: &Heap,
    sym: Symbol,
    inputs: &[Option<Ty>],
    ctx: &Ctx,
) -> Option<Ty> {
    if inputs.iter().all(Option::is_none) {
        return None;
    }
    let key = (sym, inputs.to_vec());
    if let Some(hit) = SPECIAL_MEMO.with(|m| m.borrow().get(&key).cloned()) {
        return hit;
    }
    if !SPECIAL_FUEL.with(|f| f.get() > 0) {
        return None;
    }
    let Some(_guard) = SpecializeGuard::enter(sym) else {
        REFUSALS.with(|c| c.set(c.get().saturating_add(1)));
        return None;
    };
    let Some((arms, stable)) = fixed_arms_of(heap, sym, ctx) else {
        // Nothing to re-type. For a builtin or an unknown name that is a fact about the
        // image; for a SAME-FILE global it is a fact about the moment — same-file inference has not
        // recorded its form yet, and a memoized `None` here would outlive the fixpoint
        // that records it (the bug: `step`'s first call, typed before its `defn`, pinned
        // every later specialization of it to `None`).
        // `ctx` may be a bare `Ctx::default()` — `infer_sig`'s body walk — which knows
        // no file globals, so "not a file global" is not enough: the answer is memoized
        // only for a name the IMAGE binds (a builtin, a loaded non-closure).
        if !ctx.is_file_global(sym) && super::deps::obs_global(heap, sym).is_some() {
            memo_specialization(key, None);
            NO_ARMS.with(|m| {
                m.borrow_mut().insert(sym);
            });
        }
        return None;
    };
    // A SELF-RECURSIVE body is specialized by a FIXPOINT over the recursive calls'
    // argument types and the result — see [`specialize_recursive`]. Binding the parameters
    // to the first call's argument types and skipping the recursive branch would type an
    // accumulator loop from its FIRST base case — `(sum-acc xs 0)` as `0`, where later
    // iterations return the `number` the accumulator has grown into — so without the
    // ascent the flat inference (`-> any` for every accumulator loop) would stand.
    if arms.iter().any(|a| form_mentions(heap, a.tail, sym)) {
        let out = specialize_recursive(heap, sym, &arms, inputs, ctx);
        if out.is_some() || stable {
            memo_specialization(key, out.clone());
        }
        return out;
    }
    let mut ret: Option<Ty> = None;
    let mut matched = false;
    for arm in arms.iter().filter(|a| a.heads.len() == inputs.len()) {
        // An arm whose head pattern cannot admit a known argument does not run for this
        // call (`%match` tries the next clause), so it contributes nothing.
        let admits = arm.heads.iter().zip(inputs).all(|(&h, input)| match input {
            Some(t) => !t.clone().intersect(pattern_domain(heap, h)).is_never(),
            None => true,
        });
        if !admits {
            continue;
        }
        matched = true;
        // One unit of the file's budget per arm re-typed; past it, defer (`None`).
        if !SPECIAL_FUEL.with(|f| {
            let n = f.get();
            f.set(n.saturating_sub(1));
            n > 0
        }) {
            return None;
        }
        let mut sub = ctx.with_inferring_self(sym);
        for (&h, input) in arm.heads.iter().zip(inputs) {
            sub = bind_head(heap, sub, h, input.clone());
        }
        // The body is a fresh question, not an operand of the call that asked it.
        let refusals_before = REFUSALS.with(|c| c.get());
        let outer_nest = SPEC_ARG_NEST.with(|n| n.replace(0));
        // One untypeable arm makes the union an under-approximation — defer instead.
        let typed = super::infer::with_fresh_depth(|| expr_ty(heap, arm.tail, &sub));
        SPEC_ARG_NEST.with(|n| n.set(outer_nest));
        let Some(t) = typed else {
            // An untypeable arm makes the union an under-approximation, so the answer is
            // `None` — but for a LOADED body that is an ANSWER, and one that cost a full
            // body walk to reach. It has to be memoized under the same rule the bottom of
            // this function uses, or it is re-asked at every call site and every enclosing
            // level, each time re-walking the body: checking a one-line file that calls
            // `supervisor/start` re-typed the prelude's `get` **1864 times for 52 distinct
            // questions** and spent 470 ms doing it (KI-138). Returning through `?` here
            // skipped the memo entirely. [`infer_sig`] holds the same discipline for the
            // same reason (KI-13, ~400k body walks): one walk per distinct question, and a
            // negative answer is an answer.
            if stable && REFUSALS.with(|c| c.get()) == refusals_before {
                memo_specialization(key, None);
            }
            return None;
        };
        ret = Some(match ret {
            Some(a) => a.union(t),
            None => t,
        });
    }
    let out = if matched { ret } else { None };
    // A positive answer is always memoized. A `None` only for a LOADED body: for a
    // same-file one it is a fact about the moment — an arm whose tail names a function
    // same-file inference has not recorded yet — and the fixpoint that follows may turn it into an
    // answer. (An in-flight cycle's `None` never reaches this line: `?` returns above.)
    if out.is_some() || stable {
        memo_specialization(key, out.clone());
    }
    out
}

/// Bound on the joint fixpoint of [`specialize_recursive`]. A scalar accumulator settles
/// on its third round (`0` → `int`, then unchanged), a list builder on its fourth (a
/// literal seed widens, then the element type, then the result catches up), a tuple
/// whose slots feed each other one later; two rounds of slack on top of that.
const MAX_RECURSIVE_ITERS: usize = 6;

/// [`specialized_ret`] for a **self-recursive** function: the result of `sym` called with
/// arguments of types `inputs`, as the least fixpoint of the parameters and the result
/// TOGETHER.
///
/// The parameters start at the call's argument types and the result at `never`. Each
/// round binds the parameters to their current types and the function's own name to the
/// arrow `(any… -> R)` (so a self-call in a NESTED position — `(cons x (self …))` — types
/// as `R`, while one in a branch-result position still contributes ⊥ through
/// `infer::branch_union`), reads the type of every argument at every self-call site into
/// the parameters, and the tail's type into `R`. It stops when a round changes nothing,
/// and DECLINES (`None`, the flat answer) if [`MAX_RECURSIVE_ITERS`] rounds have not
/// converged, if a site's argument or the tail cannot be typed, or if the call's arity
/// fits more than one arm — no partial answer ever leaks.
///
/// **Soundness**, by induction on an activation's recursion depth — the deepest chain of
/// recursive activations beneath it, finite for any activation that returns. Let `(P*, R*)`
/// be the fixpoint. The initial activation's arguments lie in `inputs ⊆ P*`. An activation
/// whose parameters lie in `P*` makes self-calls whose arguments, typed under `P*` by
/// `expr_ty` (a proven over-approximation, `soundness_oracle`), lie in the next round's
/// parameters — which at the fixpoint are `P*` again; so EVERY activation's parameters
/// lie in `P*`. A depth-0 activation executes no self-call, so its value comes from a
/// non-self sub-form typed under `P*` — in `R*`. A depth-`d+1` activation's self-calls
/// return values in `R*` (the hypothesis); its tail under `P*` with the self-call typed
/// `R*` is `F(R*) = R*`. So `R*` contains every return, whatever the depth. No
/// monotonicity of `expr_ty` is needed: only that it over-approximates under the binding
/// it is given, and that the loop stopped at a genuine fixpoint.
///
/// `(sum-to 10 0)` under `(defn sum-to (i acc) (if (= i 0) acc (sum-to (- i 1) (+ acc
/// i))))` is `int`: round 1 reads `acc` as `0` and `R` as `0`; round 2 finds the self-call
/// passing `(+ acc i)` — `int` under `acc : 0, i : 10` — so `acc : int` and `R : int`;
/// round 3 changes nothing. `(my-map '(1 2 3) inc)` under the classic `cons`/`first`/`rest`
/// definition is `nil | list<int>`, where the flat answer was a bare `list`.
fn specialize_recursive(
    heap: &Heap,
    sym: Symbol,
    arms: &[FixedArm],
    inputs: &[Option<Ty>],
    ctx: &Ctx,
) -> Option<Ty> {
    let arity = inputs.len();
    let mut fitting = arms.iter().filter(|a| a.heads.len() == arity);
    let arm = fitting.next()?;
    if fitting.next().is_some() {
        return None;
    }
    if self_call_sites(heap, arm.tail, sym, arity, ctx).is_empty() {
        // The self-mention is not a call of this arity (a reference, a call of another
        // arity, a quoted symbol): nothing here to iterate over.
        return None;
    }
    let bind = |params: &[Option<Ty>], ret: Ty| -> Ctx {
        let mut sub = ctx.with_inferring_self(sym);
        for (&h, input) in arm.heads.iter().zip(params) {
            sub = bind_head(heap, sub, h, input.clone());
        }
        sub.bind(sym, Some(Ty::arrow(Sig::new(vec![Ty::ANY; arity], ret))))
    };
    let mut params: Vec<Option<Ty>> = inputs.to_vec();
    let mut ret = Ty::NEVER;
    for _ in 0..MAX_RECURSIVE_ITERS {
        // One unit of the file's budget per round, like an arm re-typed above.
        if !SPECIAL_FUEL.with(|f| {
            let n = f.get();
            f.set(n.saturating_sub(1));
            n > 0
        }) {
            return None;
        }
        let sub = bind(&params, ret.clone());
        // The body is a fresh question, not an operand of the call that asked it.
        let outer_nest = SPEC_ARG_NEST.with(|n| n.replace(0));
        let round = super::infer::with_fresh_depth(|| {
            let mut next = params.clone();
            // The sites are re-read each round: a `let` binder on the way down to one is
            // typed under the round's bindings, and a self-call's argument is typed in
            // THAT scope.
            for (args, site_scope) in self_call_sites(heap, arm.tail, sym, arity, &sub) {
                for (k, &arg) in args.iter().enumerate() {
                    // An unknown parameter stays unknown; a known one takes the union of
                    // what every site hands it, and an untypeable argument makes it
                    // unknown too (the flat retreat, never an under-approximation).
                    if let Some(current) = next[k].clone() {
                        next[k] = expr_ty(heap, arg, &site_scope).map(|t| current.union(t));
                    }
                }
            }
            let next_ret = expr_ty(heap, arm.tail, &sub);
            (next, next_ret)
        });
        SPEC_ARG_NEST.with(|n| n.set(outer_nest));
        let (next, next_ret) = round;
        // An interval that moved goes to its infinity (ADR-350): `(sum-to 10 0)`'s `i` is
        // `10`, then `9 | 10`, … — a descent the round never ends on its own.
        let next: Vec<Option<Ty>> = next
            .into_iter()
            .zip(params.iter())
            .map(|(n, p)| match (n, p) {
                (Some(n), Some(p)) => Some(n.widen_intervals_against(p)),
                (n, _) => n,
            })
            .collect();
        let next_ret = next_ret.map(|r| r.widen_intervals_against(&ret));
        // Converged only when a TYPED tail reproduces `ret` under unchanged parameters —
        // a genuine fixpoint of the round, which is all the soundness argument needs.
        // A round whose tail cannot be typed contributes nothing and the loop goes on:
        // at `(sum-to 10 0)` the parameter `i` is the singleton `10`, so `(= i 0)` is
        // provably false and the base case is DEAD in round one (`Ctx::is_dead`) — every
        // live branch is a self-call and the tail has no type until round two widens
        // `i` to `int`. A tail that never types leaves the loop to the bound below.
        if next == params && next_ret.as_ref() == Some(&ret) {
            return Some(ret);
        }
        // Unchanged parameters and an untypeable tail: every later round is this one.
        // (A parameter that went unknown — an unknown callback's result feeding an
        // accumulator — lands here on its second round.)
        if next == params && next_ret.is_none() {
            return None;
        }
        params = next;
        if let Some(r) = next_ret {
            ret = r;
        }
    }
    None
}

/// Every `(sym a₁ … aₙ)` call of exactly `arity` arguments inside `form`, as its argument
/// list paired with the SCOPE it sits in: `ctx` extended by every `let`/`letrec` binder and
/// inner `fn` parameter on the path down to it. The binders are typed as the round's
/// bindings make them — `(let (j2 (+ j dir)) … (self rows j2 …))` hands `j2` over as `int`
/// when `j` is — which is what lets the recursive call's arguments say anything: typed under
/// the parameters alone, `j2` was unbound, the parameter went unknown, and `(inc j)` read as
/// `number`. A `letrec` binds its names unknown first;
/// an inner `fn`'s parameters are unknown (a self-call inside it types its arguments to the
/// extent the outer scope decides them — sound, wider). Descends through lists, vectors and
/// maps in source order; a self-call inside quoted data is collected too, which can only
/// WIDEN a parameter (its arguments type or fail to), never narrow it.
pub(in crate::types::check) fn self_call_sites(
    heap: &Heap,
    form: Value,
    sym: Symbol,
    arity: usize,
    ctx: &Ctx,
) -> Vec<(Vec<Value>, Ctx)> {
    fn walk(
        heap: &Heap,
        form: Value,
        sym: Symbol,
        arity: usize,
        scope: &Ctx,
        out: &mut Vec<(Vec<Value>, Ctx)>,
    ) {
        match form {
            Value::Pair(_) => {}
            Value::Vector(id) => {
                for it in heap.vector(id).to_vec() {
                    walk(heap, it, sym, arity, scope, out);
                }
                return;
            }
            Value::Map(id) => {
                for (k, v) in heap.map_entries(id) {
                    walk(heap, k, sym, arity, scope, out);
                    walk(heap, v, sym, arity, scope, out);
                }
                return;
            }
            _ => return,
        }
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            for &it in &items {
                walk(heap, it, sym, arity, scope, out);
            }
            return;
        };
        if head == sym && items.len() == arity + 1 {
            out.push((items[1..].to_vec(), scope.clone()));
        }
        // A self-call under an `if` is typed in the scope its test proves — the same
        // `branch_scopes` inference reads the branches under — so `(l (rest xs) (cons
        // (first xs) acc))` in the else of `(nil? xs)` hands `acc` an element, not an
        // element-or-nil. A branch the test has killed (`Ctx::is_dead`) is not walked:
        // inference gives it ⊥, and a site typed in it would feed the parameters a
        // value no activation passes.
        if value::symbol_is(head, kw::IF) && (items.len() == 3 || items.len() == 4) {
            walk(heap, items[1], sym, arity, scope, out);
            let (then_scope, else_scope) = super::guards::branch_scopes(heap, items[1], scope);
            if !then_scope.is_dead() {
                walk(heap, items[2], sym, arity, &then_scope, out);
            }
            if let Some(&else_form) = items.get(3)
                && !else_scope.is_dead()
            {
                walk(heap, else_form, sym, arity, &else_scope, out);
            }
            return;
        }
        if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
            let Some(binds) = items.get(1).and_then(|&b| super::walk::bindings(heap, b)) else {
                for &it in &items[1..] {
                    walk(heap, it, sym, arity, scope, out);
                }
                return;
            };
            let mut inner = scope.clone();
            if value::symbol_is(head, kw::LETREC) {
                for pair in binds.chunks(2) {
                    if let Some(&Value::Sym(name)) = pair.first() {
                        inner = inner.bind(name, None);
                    }
                }
            }
            for pair in binds.chunks(2) {
                let (Some(&pat), Some(&rhs)) = (pair.first(), pair.get(1)) else {
                    continue;
                };
                walk(heap, rhs, sym, arity, &inner, out);
                let rhs_ty = expr_ty(heap, rhs, &inner);
                inner = bind_head(heap, inner, pat, rhs_ty);
            }
            for &body in &items[2..] {
                walk(heap, body, sym, arity, &inner, out);
            }
            return;
        }
        if super::walk::is_fn_head(head) {
            let mut inner = scope.clone();
            if let Some(&plist) = items.get(1) {
                for p in super::walk::fn_params(heap, plist) {
                    inner = inner.bind(p, None);
                }
            }
            for &it in &items[1..] {
                walk(heap, it, sym, arity, &inner, out);
            }
            return;
        }
        for &it in &items[1..] {
            walk(heap, it, sym, arity, scope, out);
        }
    }
    let mut out = Vec::new();
    walk(heap, form, sym, arity, ctx, &mut out);
    out
}
/// [`specialized_ret`] for an inline `(fn …)` **literal with clauses or patterns** — the
/// callback shape [`super::infer::lambda_ret`] declines. A named multi-clause `defn` in
/// the same position already gets the full clause machinery via `specialized_ret`; an
/// inline literal fell to `any`, so `(reduce toks '() (fn (((x y & acc) "+") …)
/// ((acc val) …)))` said nothing while the identical clauses under a `defn` typed
/// precisely.
///
/// Two spellings reach here:
///  - **arity-only clause literals** survive expansion as clause syntax and are read
///    directly;
///  - **pattern/guard clauses** (and a destructuring single-clause param list) were
///    already lowered to one variadic `%match` lambda by the time call sites are typed,
///    so the clauses are gone from the form itself. The lowering embeds the original
///    clause-head list in each `(%match-error (quote :fn) … (quote heads))` raise;
///    that datum's printed form is the key the surface pre-pass recorded the clauses
///    under (`Ctx::fn_literal_clauses`), and the recovery below reads them back.
///
/// Same arm discipline as `specialized_ret`: keep the arms whose head count matches
/// what the combinator supplies and whose patterns can admit the supplied types
/// ([`pattern_domain`]); bind each head's binders under its position's type
/// ([`bind_head`]); type each arm tail; union. A guard (`:when`) never narrows the
/// union — a guarded clause may fall through to a later one, and both are included.
/// `None` — the flat answer — when any kept arm is untypeable (a partial union
/// under-approximates), when no arm admits, or when a top-level variadic arm
/// (`& rest` in head position) could take the call, since its frame fill differs per
/// call. There is no memo key for a literal, so no memoization; each arm still spends
/// the file's [`SPECIAL_FUEL`] budget.
pub(in crate::types::check) fn clause_lambda_ret(
    heap: &Heap,
    fn_form: Value,
    inputs: &[Option<Ty>],
    ctx: &Ctx,
) -> Option<Ty> {
    if inputs.iter().all(Option::is_none) {
        return None;
    }
    let items = list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    let forms = match items.get(1..) {
        Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
        Some(rest) => rest,
        None => return None,
    };
    if forms.is_empty() {
        return None;
    }
    // Direct read: every form parses as a clause (the arity-only literal, un-lowered).
    let direct: Option<Vec<(Vec<Value>, Value)>> = forms
        .iter()
        .map(|&clause| {
            let arm = clause_arm(heap, clause)?;
            let tail = *arm.body.last()?;
            Some((arm.heads, tail))
        })
        .collect();
    let arms: Vec<(Vec<Value>, Value)> = match direct {
        Some(arms) => arms,
        None => recovered_literal_arms(heap, fn_form, ctx)?,
    };
    clause_arms_ret(heap, &arms, inputs, ctx)
}

/// The surface `(heads, tail)` arms of a LOWERED pattern-clause literal, recovered via
/// the `(%match-error (quote :fn) scrutinee (quote heads))` call the lowering embeds:
/// its quoted head-list datum, printed, is the key the surface pre-pass recorded the
/// literal's clauses under. `None` when no such call is found (not a lowered clause
/// fn) or the key is unknown/ambiguous.
fn recovered_literal_arms(
    heap: &Heap,
    fn_form: Value,
    ctx: &Ctx,
) -> Option<Vec<(Vec<Value>, Value)>> {
    let mut work = vec![fn_form];
    while let Some(v) = work.pop() {
        match v {
            Value::Vector(id) => work.extend(heap.vector(id).to_vec()),
            Value::Pair(_) => {
                if let Some([context, _, heads]) = super::guards::match_failure_parts(heap, v)
                    && super::walk::unquote(heap, context).is_some_and(
                        |t| matches!(t, Value::Keyword(k) if value::symbol_name_ref(k) == "fn"),
                    )
                    && let Some(heads) = super::walk::unquote(heap, heads)
                {
                    let key = crate::syntax::printer::print(heap, heads);
                    return ctx.fn_literal_clauses(&key).cloned();
                }
                if let Some(items) = list_items(heap, v) {
                    work.extend(items);
                }
            }
            _ => {}
        }
    }
    None
}

/// The union of arm-tail types over the arms of `inputs`'s arity that can admit the
/// supplied types — the shared core of [`clause_lambda_ret`]'s two paths (see there for
/// the soundness discipline).
fn clause_arms_ret(
    heap: &Heap,
    arms: &[(Vec<Value>, Value)],
    inputs: &[Option<Ty>],
    ctx: &Ctx,
) -> Option<Ty> {
    let mut ret: Option<Ty> = None;
    let mut matched = false;
    for (heads, tail) in arms {
        let variadic = heads.iter().any(|&h| {
            matches!(h, Value::Sym(s)
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST))
        });
        if variadic {
            // A variadic arm can take this call whatever its fixed heads say —
            // typing around it would drop a reachable result from the union.
            return None;
        }
        if heads.len() != inputs.len() {
            continue; // a different arity's arm never runs for this call
        }
        let admits = heads.iter().zip(inputs).all(|(&h, input)| match input {
            Some(t) => !t.clone().intersect(pattern_domain(heap, h)).is_never(),
            None => true,
        });
        if !admits {
            continue;
        }
        matched = true;
        if !SPECIAL_FUEL.with(|f| {
            let n = f.get();
            f.set(n.saturating_sub(1));
            n > 0
        }) {
            return None;
        }
        let mut sub = ctx.clone();
        for (&h, input) in heads.iter().zip(inputs) {
            sub = bind_head(heap, sub, h, input.clone());
        }
        // The body is a fresh question, not an operand of the call that asked it.
        let outer_nest = SPEC_ARG_NEST.with(|n| n.replace(0));
        let typed = super::infer::with_fresh_depth(|| expr_ty(heap, *tail, &sub));
        SPEC_ARG_NEST.with(|n| n.set(outer_nest));
        let t = typed?;
        ret = Some(match ret {
            Some(a) => a.union(t),
            None => t,
        });
    }
    if matched { ret } else { None }
}

fn memo_specialization(key: (Symbol, Vec<Option<Ty>>), out: Option<Ty>) {
    SPECIAL_MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() < MAX_SPECIAL_MEMO {
            m.insert(key, out);
        }
    });
}
