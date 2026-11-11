//! Parameter domains: the union, over a body's possible executions, of the types each
//! parameter must have — what parameter inference reads beyond a single direct call.

use super::*;

// ---- parameter DOMAINS: the union over a body's possible executions ----

/// One parameter's slot in a domain vector, indexed like `params`.
type Domain = Vec<Ty>;

/// The scope facts the domain walk carries: which names an inner binder shadows,
/// and which `let`-binders **alias** a parameter. The alias table is load-bearing:
/// the `match` lowering binds the scrutinee to a fresh `m__28` and tests *that*, so
/// without it no clause guard would ever reach the parameter it came from.
#[derive(Clone)]
pub(super) struct DomainScope {
    pub(super) shadowed: HashSet<Symbol>,
    pub(super) aliases: HashMap<Symbol, usize>,
    /// `let`-bound NULLARY closures — `(let (k (fn () rest…)) …)` — with the scope they were
    /// defined in. A `match` binds the code for its later clauses this way when that code is
    /// too big to splice at every failure point (`%match-build-clause`), so a `(k)` call is
    /// where the rest of the match runs. Its domain is the body's: reading it as an opaque
    /// call made a match's clause guards vanish the moment its expansion grew past the splice
    /// budget. The definition scope is kept so a `k` its own body names is the OUTER one, as
    /// `let` binds it — the walk cannot loop.
    pub(super) thunks: HashMap<Symbol, std::rc::Rc<Thunk>>,
}

/// A `let`-bound nullary closure the domain walk follows into (see `DomainScope::thunks`).
///
/// Its domain is walked ONCE and kept. Every clause of a `match` fails to its successor's
/// thunk from several points (not a vector, the wrong count, the wrong tag), and that
/// successor's body does the same to the next one: re-walking the body at each call cost
/// (failure points)^(clauses), and a 20-clause `match` over tagged tuples never finished
/// checking. Keeping it is exact, not an approximation: the body is walked in the scope it
/// was defined in, against the same parameters, so every call site gets the same answer.
pub(super) struct Thunk {
    body: Vec<Value>,
    defined_in: DomainScope,
    domain: std::cell::OnceCell<Domain>,
}

/// The body forms of `(fn () body…)` — a closure taking no arguments — else `None`.
fn nullary_fn_body(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    let items = list_items(heap, form)?;
    match items.as_slice() {
        [Value::Sym(head), params, ..] if value::symbol_is(*head, kw::FN) => {
            let empty = matches!(params, Value::Nil)
                || list_items(heap, *params).is_some_and(|p| p.is_empty());
            empty.then(|| items[2..].to_vec())
        }
        _ => None,
    }
}

/// Each parameter's **domain** (ADR-261): the set of values for which this body can run to
/// completion, over-approximated. The successor to the unconditional-demand rule,
/// and the piece that makes an inferred parameter type describe a function's actual
/// shape instead of only its straight-line uses.
///
/// The old rule credited only *unconditional* demands — a use inside any branch
/// constrained nothing, because crediting it outright would flag a caller whose
/// value never reaches that branch. That left the ordinary shape of Brood code
/// invisible:
///
/// ```lisp
/// (defn f (x) (if (string? x) (string/length x) (+ x 1)))
/// (f :kw)   ; neither branch admits a keyword — and nothing said so
/// ```
///
/// The fix is not to credit a guarded use unconditionally but to credit it *within
/// its guard*, and to union the alternatives:
///
/// > `D(if test then else)` = `D(test) ∩ ( (G ∩ D(then)) ∪ (¬G ∩ D(else)) )`
///
/// where `G` is the parameter slice the test proves ([`guards::guard_assertion`],
/// whose `ty` is a **necessary** condition for the test being true — exactly what
/// this needs) and `¬G` its complement, or `any` for a `then_only` guard that proves
/// nothing when false. With no guard on this parameter both slices are `any`, so the
/// rule degrades to `D(then) ∪ D(else)` — "whichever branch runs, one of them
/// demands this" — which is itself sound, and is what catches the example above.
///
/// **Soundness.** Every combinator widens or is exact: an unrecognised form is
/// `any`, sequenced forms intersect (both really do run), alternatives union, and
/// only a guard slice narrows — where the guard makes it a fact. So the result is a
/// *superset* of the true domain, and an argument disjoint from it is disjoint from
/// the truth: it genuinely errors. Never a false positive.
pub(super) fn param_domains(heap: &Heap, body: &[Value], params: &[Symbol], ctx: &Ctx) -> Domain {
    let scope = DomainScope {
        shadowed: HashSet::new(),
        aliases: HashMap::new(),
        thunks: HashMap::new(),
    };
    // The body's forms are sequenced: all of them run.
    let mut acc = any_domain(params.len());
    for &form in body {
        acc = meet(acc, domain_of(heap, form, params, &scope, ctx));
    }
    // A parameter whose demands conflict to `never` could never be called
    // successfully at all. Rather than flag every caller, hand back `any` — the
    // same conservative retreat the unconditional rule made.
    for t in acc.iter_mut() {
        if t.is_never() {
            *t = Ty::ANY;
        }
    }
    acc
}

fn any_domain(n: usize) -> Domain {
    vec![Ty::ANY; n]
}

/// Both run — a value must satisfy each.
pub(super) fn meet(a: Domain, b: Domain) -> Domain {
    a.into_iter().zip(b).map(|(x, y)| x.intersect(y)).collect()
}

/// One or the other runs — a value need only satisfy the one it reaches.
fn join(a: Domain, b: Domain) -> Domain {
    a.into_iter().zip(b).map(|(x, y)| x.union(y)).collect()
}

/// The domain of one form, per parameter.
///
/// Grows the stack in heap-backed segments as it recurses, like the rest of the
/// checker's walkers: a deeply-nested-but-legal body (the kernel's own deep-value
/// tests build them) must be *typed*, not blow the host's native stack.
fn domain_of(
    heap: &Heap,
    form: Value,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        domain_of_inner(heap, form, params, scope, ctx)
    })
}

fn domain_of_inner(
    heap: &Heap,
    form: Value,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    let n = params.len();
    // A vector, map or set LITERAL evaluates every element (keys too — a map key is
    // data only once evaluated, KI-70), so each element's demands hold. Returning a
    // tuple `[(- a b)]` or a record `{:k (- a b)}` is the ordinary shape of Brood
    // code, so the demand walk descends into both (KI-70's class).
    match form {
        Value::Vector(id) => {
            return heap
                .vector(id)
                .to_vec()
                .into_iter()
                .fold(any_domain(n), |acc, it| {
                    meet(acc, domain_of(heap, it, params, scope, ctx))
                });
        }
        Value::Set(id) => {
            return heap
                .set_elems(id)
                .into_iter()
                .fold(any_domain(n), |acc, it| {
                    meet(acc, domain_of(heap, it, params, scope, ctx))
                });
        }
        Value::Map(id) => {
            return heap
                .map_entries(id)
                .into_iter()
                .fold(any_domain(n), |acc, (k, v)| {
                    let acc = meet(acc, domain_of(heap, k, params, scope, ctx));
                    meet(acc, domain_of(heap, v, params, scope, ctx))
                });
        }
        _ => {}
    }
    let Some(items) = list_items(heap, form) else {
        return any_domain(n); // an atom demands nothing on its own
    };
    let Some(&head) = items.first() else {
        return any_domain(n);
    };
    let Value::Sym(h) = head else {
        // A computed callee `((f) x …)` — the operands still all evaluate.
        let acc = items[1..].iter().fold(any_domain(n), |acc, &arg| {
            meet(acc, domain_of(heap, arg, params, scope, ctx))
        });
        // A KEYWORD in call-head position is NOT a demand, deliberately. `(:end
        // a)` does raise on anything but a map, a set or nil, so `nil | map | set` would be a
        // sound domain for `a` — and it moved ONE signature over std (std reaches for `get`)
        // while manufacturing 40 strict findings in bedit: every unsigged function that reads
        // a field of a model and hands it to a declared `model` parameter, because strict
        // reads a positively-known bound by inclusion. That is the "declare the whole program
        // at once" trap ADR-326 removed for map SHAPES, re-entered through the commonest
        // idiom in the language. The plain-mode gain (a caller passing an int to a
        // field-reading function) is not worth that; measured before deciding.
        return acc;
    };
    // Nothing here runs against the parameters now: quoted data, a closure body
    // (deferred), a definer, or a `try` — whose failure is caught, so what its body
    // demands is not a requirement on the caller.
    if value::symbol_is(h, kw::QUOTE)
        || value::symbol_is(h, kw::QUASIQUOTE)
        || value::symbol_is(h, kw::FN)
        || value::symbol_is(h, kw::TRY)
        || value::symbol_is(h, kw::TRY_PRIM)
        || value::symbol_is(h, kw::DEF)
        || value::symbol_is(h, kw::DEFN)
        || value::symbol_is(h, kw::DEFMACRO)
        || value::symbol_is(h, kw::DEFDYN)
    {
        return any_domain(n);
    }
    // The `match` compiler's failure branch: `(%raise (%match-error …))`. Reaching it
    // is not an execution any caller can want — every value that lands there raises.
    // So its domain is `never`, which is what makes a `match`'s clause patterns (and
    // a `defn` clause's `:when` guard, which lowers through the same engine) add up
    // to the function's domain instead of vanishing.
    if super::super::guards::is_raise_head(h)
        && items.len() == 2
        && super::super::guards::match_failure_parts(heap, items[1]).is_some()
    {
        return vec![Ty::NEVER; n];
    }
    // `if` — the rule this whole function exists for.
    if value::symbol_is(h, kw::IF) {
        let Some(&test) = items.get(1) else {
            return any_domain(n);
        };
        let d_test = domain_of(heap, test, params, scope, ctx);
        let (then_slice, else_slice) = guard_slices(heap, test, params, scope, ctx);
        let d_then = items
            .get(2)
            .map(|&t| domain_of(heap, t, params, scope, ctx))
            .unwrap_or_else(|| any_domain(n));
        let d_else = items
            .get(3)
            .map(|&e| domain_of(heap, e, params, scope, ctx))
            .unwrap_or_else(|| any_domain(n));
        return meet(
            d_test,
            join(meet(then_slice, d_then), meet(else_slice, d_else)),
        );
    }
    // `cond` / `when` — `if`s in disguise, and *not* always expanded away
    // before this runs: a prelude closure keeps its body as written, so the checker
    // reads `(cond …)` verbatim there. (Reading it as an ordinary call is what made
    // `type-matches?` demand a `seqable` first argument: every clause body, including
    // `(first t)`, looked unconditional.)
    if value::symbol_is(h, kw::COND) {
        return cond_domain(heap, &items[1..], params, scope, ctx);
    }
    if value::symbol_is(h, kw::WHEN) {
        let Some(&test) = items.get(1) else {
            return any_domain(n);
        };
        let d_test = domain_of(heap, test, params, scope, ctx);
        let (then_slice, else_slice) = guard_slices(heap, test, params, scope, ctx);
        let d_body = items[2..].iter().fold(any_domain(n), |acc, &f| {
            meet(acc, domain_of(heap, f, params, scope, ctx))
        });
        // The false outcome runs nothing and demands nothing.
        return meet(d_test, join(meet(then_slice, d_body), else_slice));
    }
    // `case` / `match`: only the scrutinee is guaranteed to run. Their clause bodies
    // are guarded by patterns this walk doesn't read (the *expanded* form is where a
    // pattern becomes a guard chain it can read — and does).
    if value::symbol_is(h, kw::CASE) || value::symbol_is(h, kw::MATCH) {
        return items
            .get(1)
            .map(|&scrutinee| domain_of(heap, scrutinee, params, scope, ctx))
            .unwrap_or_else(|| any_domain(n));
    }
    // `and` / `or` are `if`s in disguise — `(and a b)` runs `b` only when `a` holds,
    // `(or a b)` only when it doesn't. Running them through the same rule keeps a
    // narrowing conjunct (`(and (int? x) (> x 0))`) working on the un-expanded path,
    // where they survive as themselves.
    if value::symbol_is(h, kw::AND) || value::symbol_is(h, kw::OR) {
        return short_circuit_domain(
            heap,
            &items[1..],
            value::symbol_is(h, kw::AND),
            params,
            scope,
            ctx,
        );
    }
    // `let` / `letrec`: every binding RHS runs, then the body. A binder shadows a
    // same-named parameter; a binder whose RHS *is* a parameter aliases it.
    if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
        let Some(&binds_form) = items.get(1) else {
            return any_domain(n);
        };
        let Some(binds) = list_items(heap, binds_form) else {
            return any_domain(n);
        };
        if binds.len() % 2 != 0 {
            return any_domain(n);
        }
        let mut inner = scope.clone();
        let mut acc = any_domain(n);
        let mut i = 0;
        while i < binds.len() {
            // A destructuring binder hides names we can't enumerate — bail to `any`
            // rather than let a shadowed parameter leak a demand.
            let Value::Sym(name) = binds[i] else {
                return any_domain(n);
            };
            acc = meet(acc, domain_of(heap, binds[i + 1], params, &inner, ctx));
            match param_index(binds[i + 1], params, &inner) {
                Some(idx) => inner.aliases.insert(name, idx),
                None => inner.aliases.remove(&name),
            };
            match nullary_fn_body(heap, binds[i + 1]) {
                Some(body) => {
                    let defined_in = inner.clone();
                    inner.thunks.insert(
                        name,
                        std::rc::Rc::new(Thunk {
                            body,
                            defined_in,
                            domain: std::cell::OnceCell::new(),
                        }),
                    );
                }
                None => {
                    inner.thunks.remove(&name);
                }
            }
            inner.shadowed.insert(name);
            i += 2;
        }
        for &b in &items[2..] {
            acc = meet(acc, domain_of(heap, b, params, &inner, ctx));
        }
        return acc;
    }
    // `do`: every form runs.
    if value::symbol_is(h, kw::DO) {
        return items[1..].iter().fold(any_domain(n), |acc, &f| {
            meet(acc, domain_of(heap, f, params, scope, ctx))
        });
    }
    // A call of a `let`-bound nullary closure — a `match`'s failure thunk — runs its body:
    // its domain is the body's, in the scope the closure was defined in, walked once (`Thunk`).
    if items.len() == 1
        && let Some(thunk) = scope.thunks.get(&h).cloned()
    {
        return thunk
            .domain
            .get_or_init(|| {
                thunk.body.iter().fold(any_domain(n), |acc, &form| {
                    meet(acc, domain_of(heap, form, params, &thunk.defined_in, ctx))
                })
            })
            .clone();
    }
    // An unexpanded **macro** call: its operands are syntax, not necessarily code —
    // a template may drop them, defer them, or bind them. Nothing here is known to
    // run, so nothing is demanded. (The generic rule below assumes every operand
    // evaluates, which is true of a call and false of a macro.)
    if super::walk::resolves_to_macro(heap, ctx, h) {
        return any_domain(n);
    }
    // An ordinary call: every argument evaluates, and a parameter passed *directly*
    // to a callee whose signature is known takes the type that position demands.
    // Table lookups only (primitive / curated / declared) — never `infer_sig`, so
    // this can't recurse into the inference that called it.
    // A **file-local** `(sig …)` first: the file is checked before it loads, so its
    // declarations are on `ctx` and NOT in the heap — `declared_heap_sig` cannot see
    // them. Without this a signature constrained its callers in every OTHER module and
    // not in its own, which is where most calls to it are. `std/bytes.blsp` declares
    // `(sig at (bytes int -> int))` and then calls `(bytes/at bs off)` three lines
    // later; `off` still inferred as the arithmetic domain rather than `int`.
    //
    // A lexical local shadows the global, so its type is not the declared one; and this
    // stays a table lookup like the three below, never `infer_sig`, so it cannot recurse
    // into the inference that called it.
    // …and a same-file function's INFERRED sig (same-file inference's fixed point) right after: a
    // table lookup on `ctx`, so still no recursion — and the largest single source of
    // demands the corpus was missing, since most calls in a module are to its own
    // functions. Its domain over-approximates the callee's valid arguments, so a caller
    // intersecting with it keeps over-approximating: sound, and it only narrows across
    // the iterations.
    let callee_sig = (!scope.shadowed.contains(&h))
        .then(|| ctx.declared_sig(h))
        .flatten()
        .or_else(|| {
            (!scope.shadowed.contains(&h))
                .then(|| ctx.inferred_fn_sig(h))
                .flatten()
        })
        .or_else(|| primitive_sig(heap, h))
        .or_else(|| curated_sig(h))
        .or_else(|| declared_heap_sig(heap, h))
        // …and last, a LOADED module's inferred sig (`sig_of` → `infer_sig`): memoized, and
        // `InferGuard` answers `None` on re-entry, so the inference that called this walk
        // can never be re-entered through it. A caller's parameter handed to another
        // module's function takes that function's inferred demand — the last mechanical
        // source of `any` the corpus had. Not for this file's own names: those are on
        // `ctx` above, and the heap may hold a stale copy.
        .or_else(|| {
            (!scope.shadowed.contains(&h) && !ctx.is_file_global(h))
                .then(|| sig_of(heap, h))
                .flatten()
        });
    // Ability-op occurrence typing (ADR-190): a call to a *sealed* ability op demands
    // its first argument be a member of that ability. Only when the head IS that op
    // (`is_ability_op`): the domain is keyed by the op's bare name, and `std/tempo`
    // defines its own `->iso` beside `datetime`'s sealed `Temporal` op of that name —
    // `(->iso t)` there is tempo's function, and demanding a `datetime` member of `t`
    // typed `explain`'s parameter as one (surfaced the moment `parse!` stopped answering
    // `any`).
    let op_domain = (!scope.shadowed.contains(&h)
        && super::protocol::is_ability_op(heap, ctx.ability(), h))
    .then(|| super::protocol::sealed_op_domain(h))
    .flatten();
    let mut acc = any_domain(n);
    // **A parameter in call-HEAD position is callable.** `(g x)` only runs if `g` is,
    // so an accepted call proves it — the same argument every other demand rests on.
    // This is what types a *callback* parameter, which is what most higher-order
    // functions take and the position an argument-order slip reverses: without it
    // `(defn each (f xs) (f (first xs)))` types `f` as `any`, and passing the sequence
    // first is accepted in silence.
    //
    // Callable is `fn | native | keyword`, not just `fn`: a keyword is a function of a
    // map in Brood (`(:a {:a 1})` → 1), while maps, vectors and strings are not
    // callable (verified, not assumed — each raises).
    if let Some(pos) = param_index(head, params, scope) {
        let callable = Ty::of(Tag::Fn)
            .union(Ty::of(Tag::Native))
            .union(Ty::of(Tag::Keyword));
        acc[pos] = acc[pos].clone().intersect(callable);
    }
    // `(fold coll init f)` / `(reduce coll init f)` with a KNOWN `f`: the init is `f`'s
    // first argument and every element of `coll` its second, so a parameter in either
    // slot takes `f`'s demand — `(fold more x +)` wants `x` numeric and `more` a seqable
    // of numerics. This is the callback demand the suggested signatures were missing most.
    // Positions from `combinator_args` — the single statement of the data-first
    // convention (ADR-308), so this cannot drift against the signatures above.
    if (value::symbol_is(h, "fold") || value::symbol_is(h, "reduce")) && items.len() == 4 {
        let (coll_arg, f_arg) = match combinator_args(&items) {
            Some(pair) => pair,
            None => (items[1], items[3]),
        };
        if let Value::Sym(f) = f_arg {
            let known = !scope.shadowed.contains(&f) && param_index(f_arg, params, scope).is_none();
            let f_sig = if known {
                ctx.declared_sig(f)
                    .or_else(|| primitive_sig(heap, f))
                    .or_else(|| curated_sig(f))
                    .or_else(|| declared_heap_sig(heap, f))
            } else {
                None
            };
            if let Some(f_sig) = f_sig {
                if let Some(pos) = param_index(items[2], params, scope)
                    && let Some(p0) = f_sig.param(0)
                {
                    acc[pos] = acc[pos].clone().intersect(p0);
                }
                if let Some(pos) = param_index(coll_arg, params, scope)
                    && let Some(p1) = f_sig.param(1)
                {
                    acc[pos] = acc[pos].clone().intersect(seqable_of(p1));
                }
            }
        }
    }
    for (i, &arg) in items[1..].iter().enumerate() {
        if let Some(pos) = param_index(arg, params, scope) {
            if let Some(expected) = callee_sig.as_ref().and_then(|s| s.param(i)) {
                acc[pos] = acc[pos].clone().intersect(expected);
            }
            if i == 0
                && let Some(dom) = op_domain.clone()
            {
                acc[pos] = acc[pos].clone().intersect(dom);
            }
        }
        acc = meet(acc, domain_of(heap, arg, params, scope, ctx));
    }
    acc
}

/// `(and a b c…)` / `(or a b c…)` — each operand after the first runs only when the
/// ones before it went the right way. Recurses over the tail rather than rebuilding
/// a form, so `c`'s demand is guarded by `b`'s outcome as well as `a`'s (folding the
/// whole tail together instead would *narrow* the domain — the unsound direction).
fn short_circuit_domain(
    heap: &Heap,
    operands: &[Value],
    is_and: bool,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    let n = params.len();
    match operands {
        [] => any_domain(n),
        [only] => domain_of(heap, *only, params, scope, ctx),
        [first, rest @ ..] => {
            let d_first = domain_of(heap, *first, params, scope, ctx);
            let d_rest = short_circuit_domain(heap, rest, is_and, params, scope, ctx);
            let (then_slice, else_slice) = guard_slices(heap, *first, params, scope, ctx);
            // `and` continues when the first operand holds, `or` when it doesn't; the
            // other way, the tail never runs and demands nothing (its slice alone).
            let (taken, skipped) = if is_and {
                (then_slice, else_slice)
            } else {
                (else_slice, then_slice)
            };
            meet(d_first, join(meet(taken, d_rest), skipped))
        }
    }
}

/// `(cond t1 b1 t2 b2 … else bn)` — the `if` rule, applied down the clause list.
/// A clause's body is credited only within the slice its own test proves, and the
/// tail (no clause matched, so nothing ran) demands nothing.
fn cond_domain(
    heap: &Heap,
    clauses: &[Value],
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    let n = params.len();
    match clauses {
        [] | [_] => any_domain(n), // exhausted, or a malformed dangling test
        [test, body, rest @ ..] => {
            let d_test = domain_of(heap, *test, params, scope, ctx);
            let (then_slice, else_slice) = guard_slices(heap, *test, params, scope, ctx);
            let d_body = domain_of(heap, *body, params, scope, ctx);
            let d_rest = cond_domain(heap, rest, params, scope, ctx);
            meet(
                d_test,
                join(meet(then_slice, d_body), meet(else_slice, d_rest)),
            )
        }
    }
}

/// The parameter a form refers to — directly, or through a `let` alias — unless an
/// inner binder shadows it.
fn param_index(form: Value, params: &[Symbol], scope: &DomainScope) -> Option<usize> {
    let Value::Sym(s) = form else { return None };
    if scope.shadowed.contains(&s) {
        return scope.aliases.get(&s).copied();
    }
    params.iter().position(|&p| p == s)
}

/// The two parameter slices a test splits its branches by: what a parameter must be
/// for the test to be true, and what it must be for the test to be false. `any` in
/// both slots when the test proves nothing about that parameter — which degrades the
/// `if` rule to a plain union of the branches, the sound default.
///
/// A whole-test guard gives both slices. A *conjunction* gives only then-slices:
/// every conjunct must hold for the test to be true, but its being false says only
/// that *some* conjunct failed — never that a particular one did.
pub(super) fn guard_slices(
    heap: &Heap,
    test: Value,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> (Domain, Domain) {
    let n = params.len();
    let mut then_slice = any_domain(n);
    let mut else_slice = any_domain(n);
    if let Some(guard) = super::guards::guard_assertion(heap, test, ctx) {
        if let Some(idx) = param_index(Value::Sym(guard.sym), params, scope) {
            if !guard.else_only {
                then_slice[idx] = guard.ty.clone();
            }
            if !guard.then_only {
                else_slice[idx] = guard.else_type();
            }
        }
        return (then_slice, else_slice);
    }
    for guard in super::guards::and_conjunct_guards(heap, test, ctx) {
        if let Some(idx) = param_index(Value::Sym(guard.sym), params, scope) {
            then_slice[idx] = then_slice[idx].clone().intersect(guard.ty);
        }
    }
    (then_slice, else_slice)
}
