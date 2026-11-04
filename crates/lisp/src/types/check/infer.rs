//! Type inference over expressions — expr_ty + result-typing helpers
//! (extracted from guards.rs, file-organization split).
use super::ctx::{Ctx, resolve_clause_overload_ret, resolve_overload_ret};
use super::guards::path_of;
use super::known_ops::{self, is_one_of};
use super::sigs::{declared_heap_overload, sig_of};
use super::walk::{bindings, is_fn_head, list_items};
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::{Range, Sig, Ty};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// The type of an *undeclared* global's current heap value — the cross-file half
/// of Gap A (`docs/type-gating.md`). Same current-image observation same-file
/// Gap A makes, but read from the loaded image (via `obs_global`, which also
/// records the dependency so a change re-checks the reader — ADR-125), so it
/// reaches uses in *other* files exactly as `infer_sig` already does for
/// functions. `None` for an absent global or a **function/native** value (a fn
/// global's arrow is handled by `sig_of`; a bare function name used as a value is
/// a separate concern). Callers expose the result as `dynamic_within` (the `∩`
/// relation) — never a precise `stat` — since a global is redefinable.
/// Whether `s` is named with the dynamic-variable *earmuff* convention (`*name*`,
/// at least one char between the stars). Such a global is dynamic by convention —
/// rebound over its lifetime — so the checker types a use of it as unknown rather
/// than pinning it to its current (usually default) heap value.
pub(super) fn is_earmuffed(s: Symbol) -> bool {
    let name = value::symbol_name_ref(s);
    name.len() > 2 && name.starts_with('*') && name.ends_with('*')
}

pub(super) fn global_value_ty(heap: &Heap, s: Symbol) -> Option<Ty> {
    // A **dynamic variable** (`defdyn`) must stay unknown: its heap value is only
    // the *default*, but `binding` rebinds it to any type within a dynamic extent,
    // so typing a use against the default would be unsound. The same holds for any
    // **earmuffed** global (`*name*`, the dynamic-variable naming convention) even
    // when declared with a plain `def` and rebound via `def` — e.g. `project/*root*`
    // (`(def project/*root* nil)` at load, reassigned to the real path at runtime).
    // The type philosophy makes a redefinable global `dynamic()`, so pinning it to
    // its load-time default would false-positive on every use once it is reassigned
    // (`(path-join project/*root* …)` after a `(when (nil? project/*root*) (throw))`
    // guard reads the default `nil`, not the string it actually holds).
    if heap.is_dynamic(s) || is_earmuffed(s) {
        return None;
    }
    let v = super::deps::obs_global(heap, s)?;
    // The value is what THIS process computed at load, not a fact about the program:
    // `(def- dev? (env/dev?))` holds `true` in a dev checkout and `false` in production,
    // and typing it `true` let the constant-condition lint prove `(when (dev?) …)` always
    // true — through the inferred return of every function that reads it. So an
    // observation contributes its KIND (`bool`, `int`, `string`), never the literal —
    // the literal is the run-specific part, and it is what a truthiness or literal-set
    // proof would rest on. An observed nil says nothing at all: it is the placeholder a
    // global holds until something sets it (an unset env var reads nil too), exactly
    // the case the earmuff rule above exists for.
    if matches!(v, Value::Nil) {
        return None;
    }
    let t = Ty::of(value::tag(v));
    if t.contains_tag(Tag::Fn) || t.contains_tag(Tag::Native) {
        return None;
    }
    Some(t)
}

thread_local! {
    /// [`expr_ty`]'s recursion depth on this thread. `expr_ty` and
    /// `control_flow_ty` mutually recurse into a form's nesting, and Tier-2 return
    /// inference (`infer_sig` → `expr_ty(body)`) walks whole function bodies at
    /// call sites — so a pathologically deep, usually macro-expanded form (a huge
    /// `cond`/`or`/threaded expansion) could overflow the stack. Per-thread → sound
    /// under the parallel checker.
    static EXPR_TY_DEPTH: Cell<u32> = const { Cell::new(0) };
    /// [`expr_ty`]'s memo for the walk in progress: `(form, ctx identity)` → answer.
    /// Owned by the OUTERMOST `expr_ty` call on the thread and cleared when it returns.
    /// Sound within a walk because a `Ctx` never changes under a `&Ctx` and every
    /// binding change is a fresh object with a fresh id (`ctx::CtxId`). What it fixes:
    /// `expr_ty` of a call form types its operands in more than one fallback — a
    /// `seq_aware_call_ty` that answers `None` for an unknown collection, then the
    /// declared-sig path typing the same operand again — which is 2^depth on
    /// `(first (first … x))`: 40 deep hung `nest check`, and only the depth cap ended
    /// it. One answer per (form, scope) per walk instead.
    static EXPR_MEMO: RefCell<HashMap<(crate::core::value::PairId, u64), Option<Ty>>> =
        RefCell::new(HashMap::new());
    /// Whether some `expr_ty` frame on this thread owns [`EXPR_MEMO`] right now.
    static EXPR_MEMO_OWNED: Cell<bool> = const { Cell::new(false) };
}

/// [`EXPR_MEMO`]'s size cap per walk — past it, answers are still computed, not kept.
const MAX_EXPR_MEMO: usize = 1 << 20;

/// Depth cap for [`expr_ty`]. Comfortably below the ~1900-level overflow observed,
/// small enough for the green-process coroutine stack the parallel checker runs
/// on, and far past any real form's nesting. Past it, `expr_ty` returns `None`
/// (unknown → defer) — sound: it only ever *loses* a warning, never invents one.
pub(super) const MAX_EXPR_TY_DEPTH: u32 = 128;

/// RAII depth counter for [`expr_ty`]: `enter` bumps the thread-local depth and
/// yields `None` at [`MAX_EXPR_TY_DEPTH`] (so `expr_ty` bails); `Drop` restores it.
struct DepthGuard;
impl DepthGuard {
    fn enter() -> Option<DepthGuard> {
        EXPR_TY_DEPTH.with(|d| {
            let n = d.get();
            if n >= MAX_EXPR_TY_DEPTH {
                EXPR_TY_DEPTH_HITS.with(|c| c.set(c.get() + 1));
                None
            } else {
                d.set(n + 1);
                Some(DepthGuard)
            }
        })
    }
}
impl Drop for DepthGuard {
    fn drop(&mut self) {
        EXPR_TY_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Run `f` with [`expr_ty`]'s depth counter at 0, restoring it after. For a walk that
/// is a FRESH question — a callee's body under `infer_sig`/`specialized_ret` — not an
/// operand of the form that asked it: with the caller's depth inherited, the cap tripped
/// *because of the caller*, `expr_ty` answered `None`, and `infer_sig` memoized that as
/// the callee's signature for the rest of the file. The native
/// stack is safe either way — `expr_ty` grows it in segments.
pub(super) fn with_fresh_depth<T>(f: impl FnOnce() -> T) -> T {
    let outer = EXPR_TY_DEPTH.with(|d| d.replace(0));
    let out = f();
    EXPR_TY_DEPTH.with(|d| d.set(outer));
    out
}

/// The static type of an expression form *in `ctx`*, or `None` when it can't
/// be pinned. `None` is "unknown" and is never flagged. Self-evaluating
/// literals get their exact tag; a `quote`d datum gets the datum's tag; a call
/// with a known signature gets its result type; a variable returns whatever
/// `ctx` knows about it (typically `None` for a free / global reference).
pub(super) fn expr_ty(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    EXPR_TY_VISITS.with(|c| c.set(c.get().wrapping_add(1)));
    // Bail (defer) if the type-walk is pathologically deep — overflow guard.
    let _depth = DepthGuard::enter()?;
    let key = match form {
        Value::Pair(id) => Some((id, ctx.id())),
        _ => None,
    };
    if let Some(k) = key
        && let Some(hit) = EXPR_MEMO.with(|m| m.borrow().get(&k).cloned())
    {
        return hit;
    }
    let owner = !EXPR_MEMO_OWNED.with(|o| o.replace(true));
    // The cap bounds the DEPTH; the frame's SIZE is the compiler's, and a debug frame of
    // this function times [`MAX_EXPR_TY_DEPTH`] outgrew the walker's 1 MB stacker segment
    // with call-site specialization. Grow the stack here too, so the cap is the only limit.
    let out = crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || expr_ty_inner(heap, form, ctx));
    if owner {
        EXPR_MEMO.with(|m| m.borrow_mut().clear());
        EXPR_MEMO_OWNED.with(|o| o.set(false));
    } else if let Some(k) = key {
        EXPR_MEMO.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() < MAX_EXPR_MEMO {
                m.insert(k, out.clone());
            }
        });
    }
    out
}

thread_local! {
    /// Every [`expr_ty`] entry since the last [`reset_expr_ty_visits`] — the inferencer's
    /// work meter. A body of N nodes typed once costs about N; a multiple of N is a re-walk
    /// (KI-139's 2^depth). One increment per visit, so it costs nothing to keep on.
    static EXPR_TY_VISITS: Cell<u64> = const { Cell::new(0) };
    /// How often [`expr_ty`] answered `None` because [`MAX_EXPR_TY_DEPTH`] was reached —
    /// sound (unknown), and reported per file so a nest the checker gave up on is not read
    /// as one it checked.
    static EXPR_TY_DEPTH_HITS: Cell<u64> = const { Cell::new(0) };
}

/// Visits since the last reset (see [`EXPR_TY_VISITS`]).
pub(super) fn expr_ty_visits() -> u64 {
    EXPR_TY_VISITS.with(Cell::get)
}

/// Depth-cap hits since the last reset (see [`EXPR_TY_DEPTH_HITS`]).
pub(super) fn expr_ty_depth_hits() -> u64 {
    EXPR_TY_DEPTH_HITS.with(Cell::get)
}

/// Zero the visit meter — `check_file` does this alongside the sig memo, so a reading is
/// per file.
pub(super) fn reset_expr_ty_visits() {
    EXPR_TY_VISITS.with(|c| c.set(0));
    EXPR_TY_DEPTH_HITS.with(|c| c.set(0));
}

fn expr_ty_inner(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    match form {
        // A bare symbol is a variable reference — looked up in the local ctx
        // (let-bound RHS / if-guard narrowing). A miss falls back to a `(sig x T)`
        // value-type declaration on a *global*: a redefinable global with a declared
        // type contributes `T` to the disjointness check, so `(string/length g)` for
        // `(sig g int)` is caught. Sound — the disjointness check only warns on a
        // provable mismatch with the declared (contract) type and defers on overlap,
        // exactly `dynamic(T)`'s behaviour (contract #4). A lexical local shadows it.
        Value::Sym(s) => ctx.get(s).or_else(|| {
            if ctx.is_lexical_local(s) {
                None
            } else {
                // Declared value type first (authoritative), then the Gap A
                // inferred current-image type: same-file (`inferred_value_ty`),
                // then cross-file (`global_value_ty`, read from the loaded image).
                // All feed the gradual relation, so it's reload-safe. A name this
                // file redefines skips the heap read — the image's binding is the
                // OLD value (the file is checked pre-load; a def always wins).
                // …and a value sig declared in ANOTHER file, read from the same heap store
                // a cross-module arrow sig comes from: `(sig test/*wait-ms* int)` beside
                // the `defdyn` types a deadline built from it in every test file, where a
                // dynamic is otherwise unknown — the declaration is the author's contract
                // on every `binding` of it (ADR-259), as it is on a `def`.
                ctx.declared_value_ty(s)
                    .or_else(|| ctx.inferred_value_ty(s))
                    .or_else(|| {
                        (!ctx.is_file_global(s))
                            .then(|| {
                                super::sigs::declared_heap_value_ty(heap, s)
                                    .or_else(|| global_value_ty(heap, s))
                            })
                            .flatten()
                    })
            }
        }),
        // A vector literal `[a b c]` — its elements are evaluated in place, so
        // (ADR-128) its exact per-position types are known, not just their
        // union: infer a tuple shape rather than widening to a uniform
        // `vector<E>`. Sound and strictly more precise (a `tuple` is already a
        // subtype of the corresponding uniform `vector<E>` — `Ty::is_subtype`
        // derives that fallback — so every check that passed under the old
        // widened inference still passes). An unknown element is `any` in ITS
        // slot: the literal's arity is a fact whatever its elements are, so
        // `[row col]` over untyped params is a 2-tuple, which is what a
        // `(tuple int int)` parameter wants (the unknown slots read gradually,
        // ADR-325) and what a 3-tuple parameter must reject. Falling back to a
        // bare `vector` on one unknown element — the all-or-nothing rule
        // `element_union` has — threw the arity away with the element.
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let elems: Vec<Ty> = items
                .iter()
                .map(|&it| expr_ty(heap, it, ctx).unwrap_or(Ty::ANY))
                .collect();
            Some(Ty::tuple_of(elems))
        }
        // A set literal `#{a b …}` — `set<a | b | …>`; `#{}` is `set<never>`, the set
        // with no element type to speak of, which every `set<T>` admits. An element
        // whose type is unknown widens the whole set to the unrefined `set`.
        Value::Set(id) => {
            let items = heap.set_elems(id);
            let elems: Option<Vec<Ty>> = items.iter().map(|&it| expr_ty(heap, it, ctx)).collect();
            Some(match elems {
                Some(e) => Ty::set_of(e.into_iter().fold(Ty::NEVER, Ty::union)),
                None => Ty::of(Tag::Set),
            })
        }
        // A map literal `{:k v …}` — every keyword-literal key is definitely
        // present (it's data, evaluated once), so infer a record shape: each
        // resolvable `:key value` pair becomes a *required* field (Step 5+,
        // ADR-115). A non-keyword key, or a value whose type is unknown, is
        // simply omitted from the shape — records are open, so silently
        // under-declaring a field is sound (widens what the type claims),
        // never a false positive. `{}` / an all-unknown map infers the empty
        // record (equivalent to flat `map` for every check that matters here).
        Value::Map(id) => {
            // **Closed only when the whole literal was read** (ADR-264). A closed shape
            // asserts that every *other* key is absent, which is exactly true of a
            // literal whose entries could all be typed — `(get {:a 1} :b)` really is
            // `nil`. But a non-keyword key, or a value whose type is unknown, means an
            // entry was dropped from the shape, and claiming it absent would be a lie
            // the checker could warn on. Those infer OPEN.
            let entries = heap.map_entries(id);
            // A TABLE — no keyword key at all, every key and value typeable — is a
            // `map<K, V>` rather than a record with no fields: `{48 0 49 1 …}` keyed by
            // codepoints is `map<int, int>`, so a `get` on it answers `nil | int` where the
            // fieldless open record answered `any`. Exact for what it claims (every key is
            // a `K`, every value a `V`); which keys are PRESENT it does not claim.
            if !entries.is_empty() && entries.iter().all(|(k, _)| !matches!(k, Value::Keyword(_))) {
                let keyed: Option<Vec<(Ty, Ty)>> = entries
                    .iter()
                    .map(|&(k, v)| Some((expr_ty(heap, k, ctx)?, expr_ty(heap, v, ctx)?)))
                    .collect();
                if let Some(keyed) = keyed {
                    let (ks, vs) = keyed
                        .into_iter()
                        .fold((Ty::NEVER, Ty::NEVER), |(ks, vs), (k, v)| {
                            (ks.union(k), vs.union(v))
                        });
                    return Some(Ty::map_of(ks, vs));
                }
            }
            let mut fields = std::collections::BTreeMap::new();
            let mut complete = true;
            for (k, v) in entries {
                match (k, expr_ty(heap, v, ctx)) {
                    (Value::Keyword(name), Some(vty)) => {
                        fields.insert(name, (vty, true));
                    }
                    _ => complete = false,
                }
            }
            Some(if complete {
                Ty::record_of(fields)
            } else {
                Ty::record_of_open(fields)
            })
        }
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            // A guard-narrowed path wins over any structural result type: if this
            // whole form is a recognised access path (`(get …)`/`(nth …)`/
            // `(first …)`…) that an enclosing `(if (pred? <path>) …)` narrowed,
            // that's the most specific type for the branch (occurrence typing,
            // sound under immutability). Subsumes the per-accessor rules below.
            // …INTERSECTED with what the form reads structurally: a guard is recorded in
            // whatever scope the `if` was met in, and an inference-side scope may not know
            // the base's type at that point, so the narrowing alone can be wider than the
            // read — `(if (:n state) …)` recorded `false | nil` for the else branch where
            // the field is `nil | int`. Occurrence typing is the meet of the two.
            if let Some((base, keys)) = path_of(heap, form)
                && !keys.is_empty()
                && let Some(t) = ctx.path_ty(base, &keys)
            {
                let structural = expr_ty(heap, form, &ctx.without_path(base, &keys));
                return Some(match structural {
                    Some(s) => s.intersect(t),
                    None => t,
                });
            }
            // **Immediate application** `((fn (x) …) 1)`: the lambda's body typed with the
            // actual argument types — the same inference a callback gets from a HOF.
            if let Some(&head @ Value::Pair(_)) = items.first() {
                let is_lambda = list_items(heap, head)
                    .and_then(|l| l.first().copied())
                    .is_some_and(|h| matches!(h, Value::Sym(s) if is_fn_head(s)));
                if is_lambda {
                    let inputs: Vec<Option<Ty>> =
                        items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                    return lambda_ret(heap, head, &inputs, ctx);
                }
                // **A computed callee whose type is an arrow** — `((cur 1) "x")` under
                // `(sig cur (int -> (string -> int)))`, `((get handlers :k) msg)` over a
                // record of arrows: the arrow's result is the call's, as a local arrow's
                // is for `(f 1)` below. The walk checks the arguments against the same
                // arrow (`walk::check_computed_call`). Unknown, not `any`, when the head
                // has no arrow type: nothing is known of what it returns.
                if let Some(sig) = expr_ty(heap, head, ctx).as_ref().and_then(Ty::as_arrow) {
                    return Some(sig.ret.clone());
                }
            }
            match items.first().copied() {
                // **Keyword accessor** `(:key coll [default])` (ADR-165) — the same
                // record-field rule the `(get m :k)` case below applies, so the two
                // spellings type identically. Without this, `(:x p)` on a typed record
                // had NO result type, and `(string/length (:x p))` went uncaught while
                // the `get` spelling was flagged. `V | nil` for a `map<K,V>`, since a
                // key may be absent; an unknown key on a record falls through (records
                // are open, so the type is genuinely unknown, not an error).
                Some(Value::Keyword(key)) if items.len() == 2 || items.len() == 3 => {
                    let recv = expr_ty(heap, items[1], ctx);
                    // A record shape answers for EVERY key (ADR-264): the declared type
                    // for a required field, `T | nil` for an optional one, and — on a
                    // closed shape — `nil` for a key it does not declare, because the
                    // key is absent. Over a union of shapes it is the union of those.
                    if let Some(fty) = recv.as_ref().and_then(|t| t.record_field_ty(key)) {
                        return Some(fty);
                    }
                    if let Some((_, v)) = recv.as_ref().and_then(Ty::map_kv) {
                        return Some(v.clone().union(Ty::of(Tag::Nil)));
                    }
                    None
                }
                Some(Value::Sym(s)) => {
                    // A function LITERAL has an arrow type of its own — `(any… -> R)`, the
                    // body typed with its parameters unknown. Only when `R` is known: an arrow with an
                    // unknown result would read as `any` and fail every `⊆` against a
                    // declared result — a false positive, not a finding. A call site
                    // checking a lambda AGAINST a declared arrow does better than this
                    // (walk.rs types the body under the arrow's own domain).
                    if is_fn_head(s) {
                        return lambda_arrow(heap, form, ctx);
                    }
                    if value::symbol_is(s, kw::QUOTE) {
                        // A quoted LIST is data whose every element is right there: type it
                        // as `list<e1 | e2 | …>` (`'()` is `nil`), the way a vector literal
                        // already gets its per-element shape. Elements that are themselves
                        // lists/symbols type through `of_value` (a symbol is a `symbol`).
                        let d = *items.get(1)?;
                        if let Value::Pair(_) = d {
                            // …and its every position is right there too: a SHAPE.
                            let elems = list_items(heap, d)?;
                            return Some(Ty::list_shape_of(
                                elems.iter().map(|&e| quoted_datum_ty(heap, e)).collect(),
                            ));
                        }
                        return Some(Ty::of_value(d));
                    }
                    // Control-flow forms whose value is one of their sub-forms —
                    // typed by unioning the possible result positions, but *only*
                    // when the head isn't a lexical local shadowing the special form.
                    // CRITICAL for false-positive-freedom: if any contributing
                    // sub-form types to `None` (unknown), `control_flow_ty` returns
                    // `None`, so the whole form defers. A special-form head can't be
                    // a callable global, so we never confuse this with a call.
                    if !ctx.is_lexical_local(s) {
                        if let Some(t) = control_flow_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                        // …and when it CAN'T be typed, the answer is unknown — not a call.
                        // Falling through here treated `(do X)` as a call to a function
                        // named `do` and `(let (b) body)` as a call to `let`, and the call
                        // path's specializer then re-typed every "argument" — the bindings
                        // and the body — a second time. Every unknown-typed control-flow
                        // level doubled the walk below it: `supervisor-group-restart`'s
                        // 239-node body, twelve `let`/`do` levels deep, was typed with
                        // 78 910 expression visits (2^12 × its size), and every
                        // `brood file.blsp` that reached it paid ~240 ms before running
                        // (KI-139). A special-form head cannot be a callable global, so
                        // there is nothing for the call path to find.
                        if crate::eval::is_special_form(s) {
                            return None;
                        }
                    }
                    // **A call with an UNINHABITED argument never executes**, so its value is
                    // ⊥ — not unknown, because unknown is absorbing: a least fixpoint seeded at ⊥ (the caller-derived parameter pass's
                    // caller-derived parameters, `sigs::caller_derived_params`) could never
                    // rise out of it. Exact, not merely sound: the empty set of values has no
                    // result. Only for a head that EVALUATES its operands — a special form,
                    // a `fn` literal or an unexpanded macro reads them as syntax.
                    if super::walk::SPECIAL_HEAD.get(&s).is_none()
                        && !is_fn_head(s)
                        && !super::walk::resolves_to_macro(heap, ctx, s)
                        && items[1..]
                            .iter()
                            .any(|&a| expr_ty(heap, a, ctx).is_some_and(|t| t.is_never()))
                    {
                        return Some(Ty::NEVER);
                    }
                    // **Calling a variable whose type is an arrow.** A parameter
                    // declared `(sig apply-it ((int -> string) -> any))` carries a
                    // full signature, and the call `(f 1)` inside the body is where
                    // it should pay: without this the arrow was inert — the result
                    // had no type and the arguments went unchecked, so declaring one
                    // bought nothing at the only site that can use it.
                    //
                    // Checked FIRST, because a local shadows any global of the same
                    // name; `ctx.get` only answers for a variable actually in scope.
                    if let Some(sig) = ctx.get(s).as_ref().and_then(Ty::as_arrow) {
                        return Some(sig.ret.clone());
                    }
                    // A user `(sig …)` declaration is authoritative for the
                    // result type — consult it unless a *lexical* local (fn/let)
                    // shadows the name. (A file-global with a declared sig is the
                    // target, so guard on `is_lexical_local`, not `is_local`.)
                    if !ctx.is_lexical_local(s) {
                        // Type-variable sig: resolve the return type from arg types
                        // (e.g. `(sig identity (?A -> ?A))` → result = arg type).
                        if let Some(sv) = ctx.declared_sig_with_vars(s) {
                            let arg_tys: Vec<Option<Ty>> =
                                items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                            return Some(sv.resolve_ret(&arg_tys));
                        }
                        // A sig a LOADED module (or the stdlib image) declared with type
                        // variables is read further down, beside the other heap readers —
                        // AFTER the by-name rules, like a plain heap declaration: read here,
                        // `seq/find`'s `((seqable ?A) (?A -> any) -> (or ?A nil))` would
                        // replace the rule's `elem ∩ what the predicate tests | nil` with
                        // the wider `elem | nil`.
                        // An overloaded sig (ADR-116) — `(and (int -> int)
                        // (bool -> bool))` — resolves per matching arm instead
                        // of a single flat `ret`.
                        if let Some(sigs) = ctx.declared_overload(s) {
                            let arg_tys: Vec<Option<Ty>> =
                                items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                            return Some(resolve_overload_ret(sigs, &arg_tys));
                        }
                        if let Some(sg) = ctx.declared_sig(s) {
                            return Some(sg.ret);
                        }
                        // A QUALIFIED file global is the very function a by-name rule
                        // describes — `math/max` defined in `std/math.blsp` IS `math`'s
                        // `max`, not a shadow of it — and the rule, which reads the
                        // arguments, is sharper than the sig inferred over unknown
                        // parameters. Without this the module's own `(math/max lo (math/min
                        // hi x))` in `clamp` read `max`'s inferred return: the
                        // registry-derived `ordered` cover, which grows with every loaded
                        // module that registers `compare-to` — strict-clean with the file
                        // checked alone, red with the project loaded.
                        if ctx.is_file_global(s) && is_qualified(s) {
                            if let Some(t) = numeric_call_ty(heap, s, &items, ctx) {
                                return Some(t);
                            }
                            if let Some(t) = seq_aware_call_ty(heap, s, &items, ctx) {
                                return Some(t);
                            }
                        }
                        // A **same-file inferred** function sig (same-file inference): the return the
                        // checker inferred for a `(defn …)` in this file, which `sig_of`'s
                        // loaded-closure path can't see (the file isn't loaded while checked).
                        // After a declaration (authoritative), before the loaded lookup below.
                        if let Some(sg) = ctx.inferred_fn_sig(s) {
                            // …and a bare `number`: the body's arithmetic over parameters
                            // it knows only as numbers. Under THIS call's int arguments it
                            // may close over int — the prelude's own `(defn inc (n) (+ n
                            // 1))`, a file global inside `core.blsp`, so the by-name rule
                            // stands aside there, read `(inc slash)` as `number` for an int
                            // `slash`. The meet with the flat answer keeps it
                            // sound.
                            if sg.ret.is_unrefined_collection() || sg.ret == Ty::NUMBER {
                                // The body says nothing — or only "a collection" — about its
                                // result from its own parameters (a pass-through, an
                                // unconstrained accumulator, a recursive list builder whose
                                // flat answer is `nil | list`): re-type it under THIS call's
                                // argument types.
                                if let Some(t) =
                                    super::sigs::specialize_call(heap, s, &items[1..], ctx)
                                {
                                    return Some(t.intersect(sg.ret));
                                }
                            }
                            return Some(sg.ret);
                        }
                        // A self-call with NO return inferred yet — the first round of the
                        // function's own inference, the call not in branch-result position
                        // (`branch_union` skips those): Kleene from ⊥. It contributes
                        // nothing this round and reads the round's answer next; read as
                        // the unknown instead, `regex-alt`'s `(let ([rest i3] (regex-alt
                        // …)) [… i3])` started its index at `any` and stayed there.
                        if ctx.inferring_self() == Some(s) {
                            return Some(Ty::NEVER);
                        }
                        // An **ability op** with a declared `:-> RET` return type. The op
                        // is a generic `defn` whose body is the dispatch machinery, so its
                        // own inferred type is opaque — the declared return is the only
                        // static handle on what `(area shape)` yields, and flowing it here
                        // lets a call result feed the rest of inference (`(+ (area s) 1.0)`).
                        // Sound: the checker already warns when an impl's body doesn't
                        // conform to this return (`walk::check_impl_returns`), so the
                        // declared type is a contract, not a guess.
                        if let Some(info) = ctx.ability()
                            && let Some(ret) = info.op_ret_of(s)
                        {
                            return Some(ret.clone());
                        }
                        // The same handle for a MULTIMETHOD's declared `:-> RET`. A
                        // `defmulti` generic is a `defn` whose body is the dispatch
                        // machinery, so its inferred type is just as opaque as an ability
                        // op's, and the declaration is the only thing that can type
                        // `(compare-to a b)` at a call site. Sound for the same reason:
                        // `check_method_returns` warns when a method body does not conform.
                        if let Some(info) = ctx.multi()
                            && let Some(ret) = info.ret_of(s)
                        {
                            return Some(ret.clone());
                        }
                    }
                    // Sequence-aware refinements (`list`/`vector` constructors,
                    // `first`/`last`/`nth` extractors) and the integer-closed
                    // numeric rule — both when the head isn't a local shadow AND
                    // isn't redefined by this file (a file `defn nth` supersedes
                    // the by-name refinement); else the callee's flat result type.
                    if !ctx.is_local(s) && !ctx.is_file_global(s) {
                        if let Some(t) = numeric_call_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                        if let Some(t) = seq_aware_call_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                    }
                    // The heap-recorded counterpart of the `ctx.declared_overload`
                    // check above (ADR-116) — makes an overload declared in
                    // *another* module visible here too, the same way
                    // `sig_of`/`declared_heap_sig` already does for a plain
                    // single-arrow sig. Both heap reads are skipped for a name
                    // this file redefines — the image's binding (a builtin like
                    // `check`, a prelude closure) is the OLD value; using its
                    // signature manufactured false positives (the bintree bench's
                    // own `defn check` typed as the `check` builtin's list return).
                    if ctx.is_file_global(s) {
                        return None;
                    }
                    // A sig a LOADED module declared with type variables (the live image:
                    // hover, `reflect/expr-type`, a cross-module call), or one the stdlib
                    // image carries for a module this check never loaded (ADR-370) — the
                    // same declaration, resolved the same way from the arguments. After
                    // the by-name rules above, like every other heap reader: a rule is the
                    // checker's sharper knowledge of the same function. Skipped for a name
                    // this file redefines — the heap describes the old binding (ADR-123).
                    if let Some(sv) = super::sigs::declared_heap_sig_with_vars(heap, s)
                        .or_else(|| super::sigs::image_heap_sig_with_vars(heap, s))
                    {
                        let arg_tys: Vec<Option<Ty>> =
                            items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                        return Some(sv.resolve_ret(&arg_tys));
                    }
                    if let Some(sigs) = declared_heap_overload(heap, s) {
                        let arg_tys: Vec<Option<Ty>> =
                            items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                        return Some(resolve_overload_ret(&sigs, &arg_tys));
                    }
                    // A DECLARED return that says anything is the author's contract and is not
                    // sharpened by re-typing the body (ADR-259: a declaration is authoritative;
                    // `fuzzy/match` declares `(or nil map)` and callers get exactly that). A
                    // declared `any` says nothing, and the body is read as for an undeclared
                    // function — `assoc` declares `-> any` and types `vector | map` at a call.
                    if let Some(sg) = super::sigs::declared_heap_sig(heap, s)
                        && !sg.ret.is_any()
                    {
                        return Some(sg.ret);
                    }
                    match sig_of(heap, s).map(|sig| sig.ret) {
                        Some(t) if !t.is_unrefined_collection() => Some(t),
                        // A loaded function whose flat return is `any` or an unrefined
                        // collection (or that inferred nothing): re-type its body under
                        // this call's argument types — and MEET the answer with the flat
                        // one (`meet_flat`), since a curated `vector | map` can be sharper
                        // than what the body says under `any` inputs.
                        flat => meet_flat(
                            super::sigs::specialize_call(heap, s, &items[1..], ctx),
                            flat,
                        ),
                    }
                }
                _ => None,
            }
        }
        // A string literal → its singleton `str_lit` (B0 — literal-singleton
        // precision). `Ty::of_value` can't do this (no heap to read the bytes), so
        // it's handled here where the heap is in hand; int/bool/keyword singletons
        // come from `of_value` below.
        Value::Str(id) => Some(Ty::str_lit(heap.string(id))),
        // Int / Float / Keyword / Bool / Nil: self-evaluating (int/bool/keyword
        // carry their singleton via `of_value`; float/nil are flat).
        other => Some(Ty::of_value(other)),
    }
}

/// The union of the element forms' types, or `None` if empty or any element is
/// unknown (so the element type can't be pinned — stay unrefined, never wrong).
fn element_union(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    for &it in items {
        let t = expr_ty(heap, it, ctx)?;
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
    }
    acc
}

/// The static type of a **control-flow form** — one whose runtime value is one of
/// its sub-forms (`if`/`when`/`do`/`let`/`cond`/`case`/`match`/`and`/`or`). The
/// result is the union of the types at every position the form can yield from; the
/// `let` family threads each binding's RHS type into the scope used to type the body
/// (so `(let (x 5) (+ x 1))` sees `x : int`).
///
/// **False-positive discipline:** if *any* contributing sub-form types to `None`
/// (unknown), the whole form is `None` (defer) — `branch_union` enforces this by
/// short-circuiting on the first unknown. This graceful degradation is what keeps
/// the precise return-check warning only when the body type is fully pinned. Returns
/// `None` for a head that isn't one of these forms (the caller then falls through to
/// the call/sig path).
fn control_flow_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    // `(if test then else)` → ty(then) | ty(else). A two-arm `(if test then)`
    // (no else) can also yield `nil`.
    if value::symbol_is(head, kw::IF) {
        // A LITERAL condition selects its branch: `(if true a b)` is `a`, `(if nil a b)` /
        // `(if false a b)` is `b`. Only nil and false are falsy in Brood and every other
        // literal is truthy, so this is exact. Without it `(s-int (if true 1 "a"))` warned
        // on a program that is right — the union `1 | "a"` is precise, so it was checked by
        // `⊆`, and a string is no int.
        let taken = match items.get(1) {
            Some(Value::Nil) | Some(Value::Bool(false)) => Some(false),
            Some(Value::Bool(true))
            | Some(Value::Int(_))
            | Some(Value::Float(_))
            | Some(Value::Keyword(_))
            | Some(Value::Str(_)) => Some(true),
            _ => None,
        };
        // Each branch is typed in the scope its test proves (`guards::branch_scopes`) — the
        // same narrowing the walk and `gradual_of` apply, so `(let (g E) (if g g d))` (what
        // `or` expands to) reads `g` as truthy in the then-branch.
        let test = items.get(1).copied().unwrap_or(Value::nil());
        // A test that never produces a value selects no branch: the `if` is ⊥ too. Both
        // branch scopes are dead under it, and the union of nothing read as the UNKNOWN —
        // which is what a destructuring `let` over a `never` (the pattern lowering's
        // `(if (vector? g) … (throw …))`) yielded, and unknown is absorbing in a
        // fixpoint.
        if taken.is_none() && expr_ty(heap, test, ctx).is_some_and(|t| t.is_never()) {
            return Some(Ty::NEVER);
        }
        let (then_ctx, else_ctx) = super::guards::branch_scopes(heap, test, ctx);
        return match (items.len(), taken) {
            (4, Some(true)) | (3, Some(true)) => expr_ty(heap, items[2], &then_ctx),
            (4, Some(false)) => expr_ty(heap, items[3], &else_ctx),
            (3, Some(false)) => Some(Ty::of(Tag::Nil)),
            (4, None) => {
                // A dead branch (`Ctx::is_dead`) or a self-call branch contributes ⊥ (see
                // `branch_union`); if both did, defer.
                let t = (!then_ctx.is_dead()).then(|| branch_union(heap, &[items[2]], &then_ctx));
                let e = (!else_ctx.is_dead()).then(|| branch_union(heap, &[items[3]], &else_ctx));
                match (t, e) {
                    (Some(Some(a)), Some(Some(b))) => Some(a.union(b)),
                    (Some(Some(a)), None) => Some(a),
                    (None, Some(Some(b))) => Some(b),
                    (Some(Some(a)), Some(None)) if is_inferring_self_call(heap, items[3], ctx) => {
                        Some(a)
                    }
                    (Some(None), Some(Some(b))) if is_inferring_self_call(heap, items[2], ctx) => {
                        Some(b)
                    }
                    _ => None,
                }
            }
            (3, None) if then_ctx.is_dead() => Some(Ty::of(Tag::Nil)),
            (3, None) => Some(expr_ty(heap, items[2], &then_ctx)?.union(Ty::of(Tag::Nil))),
            // `(if test)` — a test and NO branches. The evaluator runs the test for effect
            // and yields `nil` whichever way it goes, so the type is exactly `nil`. It fell
            // through to `None` here, and "unknown" is contagious: the enclosing `defn`
            // degraded to `(string -> any)` and every check that depends on a pinned body
            // type went quiet on it — including the declared-return check, which is the one
            // thing that would have pointed at the missing branches. The shape is almost
            // always a paren slip, so it is exactly where the checker should be loudest,
            // not silent.
            (2, _) => Some(Ty::of(Tag::Nil)),
            _ => None,
        };
    }
    // `(try body… (catch e handler…))` → ty(last body) | ty(last handler); with no catch
    // it is a `do`. After expansion it is `(%try (fn () body…) (fn (e) handler…))`, whose
    // value is one of the two thunks' results — the same union.
    if value::symbol_is(head, kw::TRY) {
        let body = &items[1..];
        let (init, catch) = match body.last().copied() {
            Some(last) if is_catch_clause(heap, last) => (&body[..body.len() - 1], Some(last)),
            _ => (body, None),
        };
        let normal = init
            .last()
            .map_or(Some(Ty::of(Tag::Nil)), |&f| expr_ty(heap, f, ctx))?;
        return match catch {
            None => Some(normal),
            Some(c) => {
                let clause = list_items(heap, c)?;
                let handler = if clause.len() >= 3 {
                    expr_ty(heap, *clause.last()?, ctx)?
                } else {
                    Ty::of(Tag::Nil)
                };
                Some(normal.union(handler))
            }
        };
    }
    if value::symbol_is(head, kw::TRY_PRIM) && items.len() == 3 {
        let normal = lambda_ret(heap, items[1], &[], ctx)?;
        let handler = lambda_ret(heap, items[2], &[None], ctx)?;
        return Some(normal.union(handler));
    }
    // `(do … last)` → ty(last). Empty `(do)` is `nil`.
    if value::symbol_is(head, kw::DO) {
        return match items.last() {
            Some(&last) if items.len() > 1 => {
                let scope = sequence_scope(heap, &items[1..items.len() - 1], ctx);
                expr_ty(heap, last, &scope)
            }
            _ => Some(Ty::of(Tag::Nil)),
        };
    }
    // `(when test body…)` / `(unless test body…)` → ty(last body form) | nil
    // (the test failing yields `nil`). Surface form; usually already lowered to
    // `(if test (do …) nil)` by macroexpansion, but handled here for fragments.
    if value::symbol_is(head, kw::WHEN) || value::symbol_is(head, kw::UNLESS) {
        let last = *items.last()?;
        if items.len() < 3 {
            return Some(Ty::of(Tag::Nil));
        }
        return Some(expr_ty(heap, last, ctx)?.union(Ty::of(Tag::Nil)));
    }
    // `(let (bindings) body…)` / `(letrec …)` → ty(last body form),
    // typed in a scope with each binding's RHS type threaded in (sequentially).
    if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
        let binds = bindings(heap, *items.get(1)?)?;
        if binds.len() % 2 != 0 {
            return None;
        }
        let mut scope = ctx.clone();
        let mut i = 0;
        while i < binds.len() {
            // A `fn`-valued binder is typed from its callers — the later bindings and the
            // body — so a call of it types as the body does over what they hand it
            // (`walk::let_rhs_ty`; the walk binds the same arrow).
            let rhs_ty = super::walk::let_rhs_ty(heap, &binds, i, items, &scope);
            // A symbol takes its type and, when the RHS is an access path, the PATH ALIAS
            // a guard on it narrows through (`Ctx::add_path_alias`, as the walk's
            // `let_bind_scope` records it) — so the `match` compiler's `(let (el
            // (%vector-ref m 0)) (if (%eq el :ok) …))` narrows `m`'s tuple alternatives
            // under inference as it does under the walk, and a function's inferred RETURN
            // reads the arm's own positions. Bound plainly, `[:ok claims c]` inferred
            // `claims` as the union over every arm of what it matched on — `502 | map`.
            // Only the alias, deliberately: the walk's full rule (guard aliases, a
            // let-bound lambda's sig) re-infers per binding, and this inference runs
            // inside every expression the walk asks about — on `std/prelude/match.blsp`,
            // the match compiler's nest of lets, the full rule here took a 0.9 s check
            // past two minutes.
            match binds[i] {
                Value::Sym(name) => {
                    scope = scope.bind(name, rhs_ty);
                    // A symbol RHS is the same value (the walk's `let_bind_scope` rule): a
                    // narrowing of one reaches the other. One map insert, not the
                    // re-inferring half of the walk's rule ruled out below.
                    if let Value::Sym(target) = binds[i + 1] {
                        scope = scope.add_alias(name, target);
                    }
                    if let Some((base, keys)) = path_of(heap, binds[i + 1])
                        && !keys.is_empty()
                        && scope.is_lexical_local(base)
                    {
                        scope = scope.add_path_alias(name, base, keys);
                    }
                    // …and the COUNT alias (`guards::count_alias_target`, ADR-350): two
                    // symbol compares per binding, so it is not the re-inferring half of
                    // the walk's rule the note above rules out on cost.
                    if let Some(xs) = super::guards::count_alias_target(heap, binds[i + 1], &scope)
                    {
                        scope = scope.add_count_alias(name, xs);
                    }
                }
                // A destructuring binding: each positional binder takes the element type,
                // unknown where it can't be pinned, and the path it names
                // (`super::walk::bind_pattern`).
                pat => {
                    scope =
                        super::walk::bind_pattern(heap, scope, pat, binds[i + 1], rhs_ty.as_ref());
                }
            }
            i += 2;
        }
        let last = *items.last()?;
        if items.len() < 3 {
            return None;
        }
        let scope = sequence_scope(heap, &items[2..items.len() - 1], &scope);
        return expr_ty(heap, last, &scope);
    }
    // `(cond test1 res1 test2 res2 … else resN)` — union of the *result*
    // positions (every odd index from 2 onward). Surface form (post-expansion this
    // is nested `if`s, handled above); kept for fragments.
    if value::symbol_is(head, kw::COND) {
        let mut results = Vec::new();
        let mut i = 2;
        while i < items.len() {
            results.push(items[i]);
            i += 2;
        }
        if results.is_empty() {
            return None;
        }
        return branch_union(heap, &results, ctx);
    }
    // `(case key v1 res1 v2 res2 … [default])` — `key` at index 1, then `val res`
    // pairs from index 2; a lone trailing item is the default. Collect the result
    // of each pair (index 3, 5, …) plus any trailing default.
    if value::symbol_is(head, kw::CASE) && items.len() >= 4 {
        let clauses = &items[2..];
        let mut results = Vec::new();
        let mut i = 0;
        while i < clauses.len() {
            if i + 1 < clauses.len() {
                results.push(clauses[i + 1]); // the result of this `val res` pair
                i += 2;
            } else {
                results.push(clauses[i]); // a lone trailing default
                i += 1;
            }
        }
        if results.is_empty() {
            return None;
        }
        return branch_union(heap, &results, ctx);
    }
    // `(match scrutinee pat1 body1 pat2 body2 …)` — union of the arm *bodies*
    // (every result position; we ignore pattern-narrowing, which is sound — just
    // less precise). Bodies are at even offsets from index 3.
    if value::symbol_is(head, kw::MATCH) {
        let mut bodies = Vec::new();
        let mut i = 3;
        while i < items.len() {
            bodies.push(items[i]);
            i += 2;
        }
        if bodies.is_empty() {
            return None;
        }
        return branch_union(heap, &bodies, ctx);
    }
    // `(and a b … last)` → union of operand types (a falsy operand short-circuits
    // to itself, so any operand can be the value). `(or a b … last)` likewise.
    // Empty `(and)` → true; empty `(or)` → nil. Surface forms (post-expansion both
    // are `let`+`if`, handled above); kept for fragments.
    // Exactly, not "any operand": an operand that is NOT the last one is the value only
    // when it short-circuits — falsy for `and`, truthy for `or` — so it contributes that
    // half of its type alone. `(or (string/->number s) -1)` is `number | failure`, never `nil`; the
    // `nil` that made every `(or x default)` read as maybe-nil under `--strict` was this
    // union being taken over the whole operand.
    if value::symbol_is(head, kw::AND) {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Bool));
        }
        return short_circuit_union(heap, &items[1..], ctx, Ty::truthy().negate());
    }
    if value::symbol_is(head, kw::OR) {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Nil));
        }
        return short_circuit_union(heap, &items[1..], ctx, Ty::truthy());
    }
    None
}

/// The value of `(and …)` / `(or …)`: every operand but the last is the result only when
/// it short-circuits, so it contributes `ty ∩ short` (the falsy slice for `and`, the truthy
/// one for `or`); the last operand contributes its whole type. `None` if any operand is
/// unknown, as [`branch_union`].
fn short_circuit_union(heap: &Heap, operands: &[Value], ctx: &Ctx, short: Ty) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    for (i, &f) in operands.iter().enumerate() {
        let t = expr_ty(heap, f, ctx)?;
        let t = if i + 1 < operands.len() {
            t.intersect(short.clone())
        } else {
            t
        };
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
    }
    acc
}

/// The union of the types of several branch result forms, or `None` if *any* of
/// them is unknown — the graceful-degradation rule that keeps the precise
/// return-check from warning on a form whose value isn't fully pinned.
fn branch_union(heap: &Heap, forms: &[Value], ctx: &Ctx) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    let mut skipped_self = false;
    for &f in forms {
        // **Recursion inference.** When inferring a function's own signature, a
        // self-recursive call in a branch-result position contributes ⊥ to the union — by
        // induction it returns exactly the function's return type (the fixpoint), so the
        // *other* (base-case) branches determine it. Skipping it is what lets a tail-recursive
        // `(if base acc (self …))` infer its return from `base` instead of deferring on the
        // unknown self-call. Sound: the recursive branch adds nothing the fixpoint doesn't
        // already contain. If every branch is a self-call, `acc` stays `None` → defer.
        if is_inferring_self_call(heap, f, ctx) {
            skipped_self = true;
            continue;
        }
        let t = expr_ty(heap, f, ctx)?;
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
    }
    // Every branch a self-call: this form contributes ⊥ to whatever encloses it — exactly
    // what the skip above means — not unknown, which would make an enclosing `cond`
    // untypeable even when every other arm is an `int`. A function whose every path recurses never returns,
    // and `never` is its return.
    if acc.is_none() && skipped_self {
        return Some(Ty::NEVER);
    }
    acc
}

/// True when `form` is a direct call `(self …)` to the function whose signature is currently
/// being inferred (see [`Ctx::inferring_self`]). The head symbol is `closure.name`, exactly
/// what a self-call resolves to (mirrors `sigs::infer_from_single_call`'s self check).
fn is_inferring_self_call(heap: &Heap, form: Value, ctx: &Ctx) -> bool {
    let Some(self_name) = ctx.inferring_self() else {
        return false;
    };
    list_items(heap, form)
        .and_then(|items| items.first().copied())
        .is_some_and(|head| matches!(head, Value::Sym(s) if s == self_name))
}

/// Result type for the arithmetic ops the curated `(number… -> number)` sigs type
/// too widely — two sound sharpenings that both stay a *subtype* of `number`, so
/// they can only make a result more precise, never wrong:
///
/// - **Integer-closed** ("int op int → int"): an integer operation on integers
///   yields an integer (an i64 or a bignum — both fold to `Tag::Int`, see
///   `value::tag`), so `+ - * quot rem mod abs` with every operand `⊆ int` is
///   exactly `int`. This is what lets `(defn f (x) (* x x))` declared `(int -> int)`
///   not warn. `/` is EXCLUDED here: integer division can yield a float
///   (`(/ 6 2)` → `3`, `(/ 5 2)` → `2.5`).
/// - **Float-contagion** ("anything op float → float"): IEEE/tower contagion means
///   `+ - * /` with any operand *provably* `⊆ float` yields a `float`. Since `float`
///   is disjoint from `int`, this is what catches an `(int -> int)`-declared body
///   doing float arithmetic (`(+ x 1.5)` → `float`), plus the always-float unary
///   math `sqrt sin cos tan` — mismatches the flat `number` sig would silently miss.
///
/// Returns `None` (defer to the curated `number` sig) whenever a rule can't fire
/// with certainty — a non-numeric or unknown operand, a mixed int/`number` set that
/// proves neither all-int nor any-float, or zero operands (a bare `(+)`). Deferring
/// is always sound: the wider `number` never narrows below what the value can be.
fn numeric_call_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let num = Ty::NUMBER;
    let float = Ty::of(Tag::Float);

    // Always-float unary math: `sqrt`/`sin`/`cos`/`tan` return a float even for a
    // perfect square / whole-number argument (`(sqrt 4)` → `2.0`). Only fires on a
    // known numeric argument (a non-numeric one is a separate arg-type error the
    // curated sig already flags).
    //
    // `sqrt` is keyed `math/sqrt`: ADR-227 moved it into `std/math.blsp`, so the bare
    // spelling no longer exists (unbound without an import) and this rule was dead for the
    // spelling that does (`rule_heads_are_bound` guards every key).
    let is_always_float = value::symbol_is(head, "math/sqrt")
        || value::symbol_is(head, "math/sin")
        || value::symbol_is(head, "math/cos")
        || value::symbol_is(head, "math/tan");
    if is_always_float {
        let arg = *items.get(1)?;
        let t = expr_ty(heap, arg, ctx)?;
        return t.is_subtype(&num).then_some(float);
    }
    // An extremum hands back ONE OF ITS OPERANDS — `(math/max a b)` is `a` or `b` — so its
    // result is the union of the operand types, not the operator's whole domain (`ordered`,
    // which is what the registry-derived sig says and what made `(+ 1 (math/min i n))` read
    // as `number + ordered` under `--strict`). Sound: the union of the operands is exactly
    // the set of values it can return.
    //
    // And an operand the checker cannot type makes that union the UNKNOWN — `0 ∪ ?` is `?`
    // — not the sig's `ordered`. Deferring to the sig would read the same value two ways:
    // the unknown `s` passes into an `int` parameter untouched, but `(math/max 1 s)` —
    // which IS `s` or `1` — would come back positively `ordered` and warn into the same
    // parameter (every clamp over an untyped bound). It is not the
    // arithmetic case: `(dec s)` computes a NEW value that is positively a `number`
    // whatever `s` was, and `number` into `int` is the merely-wider ADR-298 exists to
    // flag; an extremum computes nothing. `Some(any)` — not `None`, which would fall to
    // the sig — reads as `dynamic_within(any)` at the call: the bare unknown.
    if is_extremum(head) {
        let args = items.get(1..)?;
        let mut acc: Option<Ty> = None;
        for &arg in args {
            let Some(t) = expr_ty(heap, arg, ctx) else {
                return Some(Ty::ANY);
            };
            acc = Some(match acc {
                Some(u) => u.union(t),
                None => t,
            });
        }
        return acc;
    }
    // A MASK bounds the conjunction whatever the other operand is: `m ∈ [0, hi]` makes
    // `(bit/and x m)` lie in `[0, hi]` (a non-negative `m` has no sign bit to keep, and
    // clearing bits never raises a value). The slot-index idiom `(bit/and i 1)` is then
    // `int[0..1]`, which is what lets `(nth [10 20] k)` read as present.
    if value::symbol_is(head, "bit/and") && items.len() == 3 {
        let bound = items[1..]
            .iter()
            .filter_map(|&arg| expr_ty(heap, arg, ctx)?.int_range())
            .filter(|r| r.lo.is_some_and(|lo| lo >= 0))
            .filter_map(|r| r.hi)
            .min()?;
        return Some(Ty::int_in(Range::new(Some(0), Some(bound))));
    }
    numeric_op_kind(head)?;
    // Every operand must be a known numeric type; one non-numeric / unknown defers.
    // (Zero operands — e.g. a bare `(+)` — also defers, leaving the curated sig.)
    let args = items.get(1..)?;
    if args.is_empty() {
        return None;
    }
    let mut tys = Vec::with_capacity(args.len());
    for &arg in args {
        tys.push(expr_ty(heap, arg, ctx)?);
    }
    // An operand that never produces a value — a call that always throws, a binder read
    // off the joint fixpoint's ⊥ seed — makes the operation one too. Deferred to the
    // operator's signature instead, `(inc never)` read `number`, and a derived index
    // that started at ⊥ ascended through `number` and never came back.
    if tys.iter().any(Ty::is_never) {
        return Some(Ty::NEVER);
    }
    numeric_result(head, &tys)
}

/// Is `sym` spelled with a module prefix (`math/max`)? A qualified file global is the
/// module's own definition of that name, never a shadow of a same-named root function.
fn is_qualified(sym: Symbol) -> bool {
    value::symbol_name_ref(sym).contains('/')
}

/// The extremum operators — the ones that answer with one of their operands rather than a
/// value computed from them. One definition, read by both the form-level
/// [`numeric_call_ty`] and the type-level [`numeric_result`] a callback goes through.
fn is_extremum(head: Symbol) -> bool {
    is_one_of(head, known_ops::EXTREMUM)
}

/// The **additive** ring operators — `+ - inc dec`, i.e. the ring minus `*`. What sets
/// them apart for [`numeric_result`] is that shifting a value by whole numbers cannot
/// change whether it has a denominator, which multiplication can (`(* 2 1/2)` is `1`).
fn is_additive(head: Symbol) -> bool {
    is_one_of(head, known_ops::ADDITIVE)
}

/// What a numeric operator is, for [`numeric_result`]: `(contagious, int_closed, ring,
/// division)`. `None` for anything that isn't one of these operators.
fn numeric_op_kind(head: Symbol) -> Option<(bool, bool, bool, bool)> {
    let is_division = value::symbol_is(head, "/");
    // `inc`/`dec` are `(+ n 1)` / `(- n 1)` in the prelude, so they close exactly as `+`
    // and `-` do — over ints, over floats (contagion), over ratios.
    let is_ring = is_one_of(head, known_ops::RING);
    let is_contagious = is_ring || is_division;
    let is_int_closed = is_ring || is_one_of(head, known_ops::INT_CLOSED_BEYOND_RING);
    (is_contagious || is_int_closed).then_some((is_contagious, is_int_closed, is_ring, is_division))
}

/// The result type of numeric operator `head` applied to operands of types `tys` — the
/// operand logic of [`numeric_call_ty`] over TYPES rather than forms, so a callback
/// (`(map inc xs)`) and a fold (`(reduce + 0 xs)`) can share it. `None` whenever a rule
/// can't fire with certainty: a non-numeric operand, or nothing more specific than
/// `number` provable for a non-contagious operator. Deferring is always sound: the wider
/// type never narrows below what the value can be.
///
/// - **Int-closure** (`+ - * quot rem mod math/abs inc dec`): every operand `⊆ int` →
///   exactly `int`. `/` is excluded: integer division is exact and yields a ratio.
/// - **Float-contagion**: `+ - * / inc dec` with any operand `⊆ float` → `float`.
/// - **Ratio-closure**: the ring operators and `/` over operands all `⊆ int | ratio` →
///   `int | ratio` (a ratio reduces: `(+ 1/2 1/2)` is the int `1`).
/// - **Additive int-plus-ratio**: `+ - inc dec` over ints and exactly ONE ratio →
///   exactly `ratio`. A `Ratio` is kept reduced and is demoted to an `Int` when its
///   denominator is 1 (`core::value`), so no ratio is ever integral, and shifting one
///   by whole numbers cannot make it integral: `n ± p/q` is `(nq ± p)/q`, whose
///   denominator is still `q > 1`. `*` and `/` are excluded — they can land on an int
///   from the same operands (`(* 2 1/2)` is `1`).
/// - Otherwise, every operand a number → `number`, still narrower than the operator's
///   declared domain (`number`, or `number | <the records with num/* methods>` — ADR-299).
pub(super) fn numeric_result(head: Symbol, tys: &[Ty]) -> Option<Ty> {
    // An extremum hands back ONE OF ITS OPERANDS, so its result is their union — the rule
    // `numeric_call_ty` applies to the written-out form. It belongs here too, because a
    // CALLBACK never reaches that path: `(fold readings 0 math/max)` asked `sig_of` instead
    // and got the operator's whole registry domain (`ordered`, ADR-299), so a fold of ints
    // read as `ordered` and its use as an int was reported.
    if is_extremum(head) {
        if tys.is_empty() {
            return None;
        }
        return tys
            .iter()
            .cloned()
            .reduce(|acc, t| acc.union(t))
            .filter(|t| t.is_subtype(&Ty::NUMBER));
    }
    let (is_contagious, is_int_closed, is_ring, is_division) = numeric_op_kind(head)?;
    if tys.is_empty() {
        return None;
    }
    let int = Ty::of(Tag::Int);
    let float = Ty::of(Tag::Float);
    let num = Ty::NUMBER;
    let ratio = Ty::of(Tag::Ratio);
    let int_or_ratio = int.clone().union(ratio.clone());
    let mut all_int = true;
    let mut all_int_or_ratio = true;
    let mut any_float = false;
    // Operands that are provably a ratio and provably not an int — the ones the
    // additive rule below counts. `int_shifted` is "every OTHER operand is an int".
    let mut strict_ratios = 0usize;
    let mut int_shifted = true;
    for t in tys {
        if !t.is_subtype(&num) {
            return None;
        }
        let is_int = t.is_subtype(&int);
        all_int &= is_int;
        all_int_or_ratio &= t.is_subtype(&int_or_ratio);
        any_float |= t.is_subtype(&float);
        if !is_int && t.is_subtype(&ratio) {
            strict_ratios += 1;
        } else if !is_int {
            int_shifted = false;
        }
    }
    if is_contagious && any_float {
        return Some(float);
    }
    if is_int_closed && all_int {
        // …within the INTERVAL the operands' intervals give (ADR-350): `(+ i 1)` over
        // `int[0..]` is `int[1..]`, `(- n 1)` over `int[4..]` is `int[3..]`; a literal
        // result is a literal. `quot`/`rem`/`mod` stay plain `int`.
        let ranges: Vec<Range> = tys
            .iter()
            .map(|t| t.int_range().unwrap_or(Range::ALL))
            .collect();
        let name = value::symbol_name(head);
        let spelled = match name.as_str() {
            "%add" => "+",
            "%sub" => "-",
            "%mul" => "*",
            other => other,
        };
        let r = match (spelled, ranges.as_slice()) {
            ("+", rs) => rs.iter().copied().reduce(Range::plus),
            ("*", rs) => rs.iter().copied().reduce(Range::times),
            ("-", [only]) => Some(Range::negated(*only)),
            ("-", [first, rest @ ..]) => {
                Some(rest.iter().fold(*first, |acc, r| Range::minus(acc, *r)))
            }
            ("inc", [only]) => Some(Range::plus(*only, Range::point(1))),
            ("dec", [only]) => Some(Range::minus(*only, Range::point(1))),
            ("math/abs", [only]) => Some(if only.lo.is_some_and(|lo| lo >= 0) {
                *only
            } else if only.hi.is_some_and(|hi| hi <= 0) {
                Range::negated(*only)
            } else {
                Range::at_least(0)
            }),
            // `(mod a b)` with `b > 0` is in `[0, b-1]` (the sign follows the divisor);
            // `(rem a b)` the same when `a ≥ 0` too; `(quot a b)` with both non-negative
            // is in `[0, hi(a) / lo(b)]`.
            ("math/mod" | "math/rem" | "math/quot", [a, b]) => {
                let positive_divisor = b.lo.is_some_and(|lo| lo >= 1);
                let non_negative_dividend = a.lo.is_some_and(|lo| lo >= 0);
                if !positive_divisor {
                    None
                } else if name.ends_with("quot") {
                    non_negative_dividend
                        .then(|| Range::new(Some(0), a.hi.zip(b.lo).map(|(ah, bl)| ah / bl)))
                } else if name.ends_with("mod") || non_negative_dividend {
                    Some(Range::new(Some(0), b.hi.map(|bh| bh - 1)))
                } else {
                    None
                }
            }
            _ => None,
        };
        return Some(match r {
            Some(r) => Ty::int_in(r),
            None => int,
        });
    }
    // `(+ 1 1/2)` is 3/2 and can be nothing else — see the doc comment above. Placed
    // before the int|ratio fallback, which is the honest answer only once more than one
    // operand can carry a denominator.
    if is_additive(head) && int_shifted && strict_ratios == 1 {
        return Some(ratio);
    }
    // A division the checker can decide exactly — narrow before the `int | ratio`
    // fallback, which is the honest answer only where the split is undecidable.
    if is_division
        && all_int_or_ratio
        && let Some(t) = division_result(tys)
    {
        return Some(t);
    }
    // Ratios close over `+ - *` exactly as ints do, and `/` is exact over them too
    // (`(/ 3/2 1/2)` → 3). The declared signature — widened to `number | map` for `Num`
    // records — is a true type for the operator but noise for an all-numeric expression. `quot`/`rem`/`mod` are integer
    // operations and stay int-only.
    if (is_ring || is_division) && all_int_or_ratio {
        return Some(int_or_ratio);
    }
    if is_contagious {
        return Some(num);
    }
    None
}

/// The exact kind of a `/` whose operands the checker can decide — the *narrow first*
/// half of the "merely-wider" residue (ROADMAP, § Merely-wider inference case). Brood's
/// division is exact, so `int / int` is genuinely `int | ratio` and a body declared
/// `int` cannot be flagged until the cases that ARE decidable stop landing in that
/// union. Two are, both off the int-literal refinement (ADR-117):
///
/// - **A literal ±1 divisor.** `(/ x 1)` is `x` and `(/ x -1)` is `-x`, so the result is
///   the numerator's kind whatever the numerator is. The numerator's *literal* set is
///   deliberately dropped rather than carried through — `(/ 6 -1)` is `-6`.
/// - **Every operand a known int literal.** Fold the division and report whether the
///   results are integral: `(/ 6 3)` is `int`, `(/ 5 2)` is `ratio`, `(/ 2)` (unary `/`
///   is the reciprocal) is `ratio`. A literal *set* landing on both keeps `int | ratio`.
///
/// `None` defers to the caller's `int | ratio`, which is always sound — it contains
/// every answer this function can give. A **zero divisor defers too**: `(/ 6 0)` raises
/// E0040, and typing an expression that cannot produce a value would be stating the
/// arithmetic instead of what the language does.
fn division_result(tys: &[Ty]) -> Option<Ty> {
    let int = Ty::of(Tag::Int);
    let ratio = Ty::of(Tag::Ratio);

    // Every divisor a literal ±1: the numerator's kind survives unchanged.
    if tys.len() >= 2
        && tys[1..].iter().all(|t| {
            t.as_lit_int()
                .is_some_and(|set| set.iter().all(|&n| n == 1 || n == -1))
        })
    {
        if tys[0].is_subtype(&int) {
            return Some(int);
        }
        if tys[0].is_subtype(&ratio) {
            return Some(ratio);
        }
    }

    // Every operand a known int literal: fold each combination exactly. The cap keeps a
    // wide enumerated type (`(or 200 404 500)`) from turning a type query into a
    // cross-product walk; over the cap the `int | ratio` answer is still correct.
    const MAX_COMBINATIONS: usize = 64;
    let mut sets: Vec<Vec<i64>> = Vec::with_capacity(tys.len());
    for t in tys {
        if !t.is_subtype(&int) {
            return None;
        }
        sets.push(t.as_lit_int()?.iter().copied().collect());
    }
    let combinations = sets
        .iter()
        .try_fold(1usize, |acc, set| acc.checked_mul(set.len()))?;
    if combinations == 0 || combinations > MAX_COMBINATIONS {
        return None;
    }

    let (mut any_int, mut any_ratio) = (false, false);
    let mut odometer = vec![0usize; sets.len()];
    loop {
        let values: Vec<i64> = sets.iter().zip(&odometer).map(|(set, &i)| set[i]).collect();
        // Unary `/` is the reciprocal — `(/ 2)` is `1/2` — so the numerator is 1 and the
        // single operand is the divisor.
        let (numerator, divisors) = if values.len() == 1 {
            (1i128, &values[..])
        } else {
            (values[0] as i128, &values[1..])
        };
        let mut denominator = 1i128;
        for &d in divisors {
            if d == 0 {
                return None;
            }
            denominator = denominator.checked_mul(d as i128)?;
        }
        if numerator % denominator == 0 {
            any_int = true;
        } else {
            any_ratio = true;
        }
        // Both kinds reachable: the union is the exact answer, which the caller already
        // gives — stop rather than walk the rest.
        if any_int && any_ratio {
            return None;
        }
        let mut place = odometer.len();
        loop {
            if place == 0 {
                return match (any_int, any_ratio) {
                    (true, false) => Some(int),
                    (false, true) => Some(ratio),
                    _ => None,
                };
            }
            place -= 1;
            odometer[place] += 1;
            if odometer[place] < sets[place].len() {
                break;
            }
            odometer[place] = 0;
        }
    }
}

/// The scope the LAST form of a body sequence is typed in, after folding every preceding
/// form's [`diverging guard`](super::walk::diverging_guard_scope) into it.
///
/// Inference types a `do`/`let` body as the type of its last form and never looked at the
/// forms before it — which is right for values (they are evaluated for effect) and wrong for
/// SCOPE: `(when (nil? root) (error …))` proves `root` is not nil for everything after it,
/// and that includes the body's result. Without this the walk narrowed (no warning inside
/// the function) while the inferred RETURN still carried the `nil`, so every caller was
/// reported instead — the same walk/inference split that let ADR-316 false-positive.
fn sequence_scope(heap: &Heap, earlier: &[Value], ctx: &Ctx) -> Ctx {
    let mut scope = ctx.clone();
    for &form in earlier {
        if let Some(next) = super::walk::diverging_guard_scope(heap, form, &scope) {
            scope = next;
        }
    }
    scope
}

/// A record shape and whether it is open — for the sinks that carry a shape forward.
struct ShapeOf {
    fields: std::collections::BTreeMap<value::Symbol, (Ty, bool)>,
    open: bool,
}

/// The record shape of a type, open or closed.
fn record_shape_of(ty: Option<&Ty>) -> Option<ShapeOf> {
    let ty = ty?;
    Some(ShapeOf {
        fields: ty.record_fields()?.clone(),
        open: ty.record_is_open() == Some(true),
    })
}

/// The fields of a **closed** record only. An open record may carry keys nothing
/// declares, so a rule reading "these are all the keys" does not hold for one.
fn closed_record_fields(
    ty: Option<&Ty>,
) -> Option<&std::collections::BTreeMap<value::Symbol, (Ty, bool)>> {
    let ty = ty?;
    if ty.record_is_open() != Some(false) {
        return None;
    }
    ty.record_fields()
}

/// Every argument as a literal keyword, or `None` if any is dynamic — in which case the
/// caller cannot say *which* fields an operation touches and must widen.
fn literal_keyword_args(args: &[Value]) -> Option<Vec<value::Symbol>> {
    args.iter()
        .map(|arg| match arg {
            Value::Keyword(name) => Some(*name),
            _ => None,
        })
        .collect()
}

/// `k1 v1 k2 v2 …` as `(literal keyword, value form)` pairs, or `None` if any key is
/// dynamic or the list is odd.
fn literal_keyword_pairs(args: &[Value]) -> Option<Vec<(value::Symbol, Value)>> {
    if !args.len().is_multiple_of(2) {
        return None;
    }
    args.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| match pair[0] {
            Value::Keyword(name) => Some((name, pair[1])),
            _ => None,
        })
        .collect()
}
/// Element-aware result type for the sequence and map builtins — `None` falls through to
/// the callee's flat signature. Each rule is sound: an element read widens with `nil`
/// for the empty / out-of-range case, and a rule only refines what it can prove.
fn seq_aware_call_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let len = items.len();
    match value::symbol_name_ref(head) {
        "list" => list_literal_ty(heap, items, ctx),
        "%record" if len == 3 => record_ctor_ty(heap, items, ctx),
        "vector" => element_union(heap, &items[1..], ctx).map(Ty::vector_of),
        "%vector-ref" if len == 3 => vector_ref_ty(heap, items, ctx),
        name @ ("first" | "second" | "third" | "last" | "nth") => {
            positional_read_ty(heap, name, items, ctx)
        }
        "seq/filter" => filter_ty(heap, items, ctx),
        "seq/find" if len == 3 => find_ty(heap, items, ctx),
        "seq/reject" if len == 3 => reject_ty(heap, items, ctx),
        "reverse" | "%map-pairs" | "sort" | "seq/sort-by" => same_elements_ty(heap, items, ctx),
        "seq/distinct" | "seq/dedupe" => deduplicated_ty(heap, items, ctx),
        "seq" if len == 2 => seq_ty(heap, items, ctx),
        name @ ("rest" | "seq/but-last") => drop_one_ty(heap, name == "rest", items, ctx),
        "take" | "drop" | "seq/take-while" | "seq/drop-while" | "seq/take-last"
        | "seq/drop-last" => {
            list_result(expr_ty(heap, *items.get(1)?, ctx).and_then(|t| t.elem_ty_union()))
        }
        "count" | "string/length" if len == 2 => count_ty(heap, items, ctx),
        "range" if len >= 2 => range_ty(heap, items, ctx),
        // `(range)` with no bound still produces numbers.
        "range" => list_result(Some(Ty::NUMBER)),
        "vec" if len == 2 => vec_ty(heap, items, ctx),
        "into" if len == 3 => into_ty(heap, items, ctx),
        "conj" if len >= 3 => conj_ty(heap, items, ctx),
        "cons" if len == 3 => cons_ty(heap, items, ctx),
        "append" => append_ty(heap, items, ctx),
        "merge" if len >= 2 => merge_ty(heap, items, ctx),
        "get" if len >= 3 => get_ty(heap, items, ctx),
        "map/get-in" if len == 3 || len == 4 => get_in_ty(heap, items, ctx),
        "keys" if len == 2 => keys_ty(heap, items, ctx),
        "vals" if len == 2 => vals_ty(heap, items, ctx),
        "dissoc" if len >= 3 => dissoc_ty(heap, items, ctx),
        name @ ("update" | "map/assoc-in" | "map/update-in") if len >= 3 => {
            update_ty(heap, name == "update", items, ctx)
        }
        "assoc" if len >= 4 && (len - 2).is_multiple_of(2) => assoc_ty(heap, items, ctx),
        "string/->number" if len == 2 || len == 3 => parse_number_ty(heap, items, ctx),
        "apply" if len >= 3 => apply_ty(heap, items, ctx),
        "map" => map_ty(heap, items, ctx),
        "seq/keep" => keep_ty(heap, items, ctx),
        "seq/interpose" if len == 3 => interpose_ty(heap, items, ctx),
        name @ ("reduce" | "fold") => reduce_ty(heap, name == "reduce", items, ctx),
        _ => None,
    }
}

/// The literal index a positional read names — `first` 0, `second` 1, `third` 2,
/// `nth` its literal non-negative index argument — or, for `last`, `Some(None)`: the
/// final position, whatever the length. `None` when the index is not literal.
fn positional_index(head: &str, items: &[Value]) -> Option<Option<usize>> {
    match head {
        "first" => Some(Some(0)),
        "second" => Some(Some(1)),
        "third" => Some(Some(2)),
        "last" => Some(None),
        _ => match items.get(2) {
            Some(Value::Int(n)) if *n >= 0 => Some(Some(*n as usize)),
            _ => None,
        },
    }
}

/// `(list a b …)` is a list of exactly those elements, each in its position — a
/// positional SHAPE (the list sibling of a vector literal's tuple), so `(first (list m
/// '(…)))` is the map and not `pair | map`. An unknown element is `any` in its slot: the
/// arity is a fact whatever the elements are. `(list)` is `nil`.
fn list_literal_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    if items.len() == 1 {
        return Some(Ty::of(Tag::Nil));
    }
    let elems: Vec<Ty> = items[1..]
        .iter()
        .map(|&item| expr_ty(heap, item, ctx).unwrap_or(Ty::ANY))
        .collect();
    Some(Ty::list_shape_of(elems))
}

/// `%vector-ref` is what a destructuring `let`/`match` lowers a `[x y w h]` binder to,
/// behind a length check; out of range it throws rather than yielding nil, so it is
/// exactly the element (a tuple position when the index is literal).
fn vector_ref_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, items[1], ctx)?;
    if let Value::Int(n) = items[2]
        && n >= 0
        && let Some(elem) = collection_ty.tuple_elem_at(n as usize)
    {
        return Some(elem);
    }
    collection_ty.elem_ty()
}

/// `first` / `second` / `third` / `last` / `nth`: the element type, widened with `nil`
/// (or `nth`'s default) unless the read is provably in range.
fn positional_read_ty(heap: &Heap, head: &str, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let arg = *items.get(1)?;
    let collection_ty = expr_ty(heap, arg, ctx)?;
    let is_nth = head == "nth";
    let nth_default = || {
        if is_nth && items.len() == 4 {
            expr_ty(heap, items[3], ctx)
        } else {
            None
        }
    };
    // A statically-known index into a tuple-typed collection resolves to that *exact*
    // position's type (ADR-128), not the coarse union every other access falls back to.
    if let Some(elems) = collection_ty.positional_elems() {
        let index = match positional_index(head, items) {
            Some(Some(index)) => Some(index),
            Some(None) => elems.len().checked_sub(1),
            None => None,
        };
        if let Some(index) = index {
            // In range → exactly that position's type, no `nil` (a tuple's arity is
            // fixed). Provably out of range → `nil`, as the runtime answers — or, for
            // `(nth coll i default)`, the default.
            return match elems.get(index) {
                Some(t) => Some(t.clone()),
                None if is_nth && items.len() == 4 => Some(nth_default().unwrap_or(Ty::ANY)),
                None => Some(Ty::of(Tag::Nil)),
            };
        }
        // A computed `nth` index whose INTERVAL lies inside the arity reads the union of
        // the positions it can name — `(nth [10 20] (bit/and i 1))` is `10 | 20`, present
        // (ADR-350's interval, applied to a shape rather than a length).
        if is_nth
            && items.len() == 3
            && let Some(range) = expr_ty(heap, items[2], ctx).and_then(|t| t.int_range())
            && let (Some(lo), Some(hi)) = (range.lo, range.hi)
            && lo >= 0
            && (hi as usize) < elems.len()
        {
            let mut out = elems[lo as usize].clone();
            for t in &elems[lo as usize + 1..=hi as usize] {
                out = out.union(t.clone());
            }
            return Some(out);
        }
    }
    // Over a UNION of shapes and sequences — `(tuple :error string) | (tuple :ok map)`,
    // the tagged pair every parser returns — the read is answered term by term (a shape
    // by its position), never by the union of every element. A guard on another position
    // has already dropped the alternatives it rules out (`Ctx::narrow_path`).
    if collection_ty.alt_terms().is_some()
        && let Some(index) = positional_index(head, items)
    {
        // An `nth` default answers wherever a term runs off its end.
        if is_nth && items.len() == 4 {
            let default = nth_default().unwrap_or(Ty::ANY);
            return collection_ty
                .element_at_over_union(index)
                .map(|t| t.union(default));
        }
        return collection_ty.element_at_over_union(index);
    }
    // A read from exactly `nil` — the then-branch of `(nil? xs)` — is `nil`, or the `nth`
    // default (ADR-350), as the runtime answers.
    if collection_ty == Ty::of(Tag::Nil) {
        return Some(nth_default().unwrap_or(Ty::of(Tag::Nil)));
    }
    let elem = collection_ty.elem_ty()?;
    // `first`/`last` of a provably non-empty list is an element, full stop; every other
    // access yields `nil` too — except `(nth coll i default)`, whose absent case IS the
    // default. …and by LENGTH (ADR-350): `second` of a collection of two or more,
    // `(nth xs 3)` of one at least four long, `(nth xs i)` where a guard established
    // `i < (count xs)` and `i`'s interval says it is not negative.
    let count = collection_ty.count_range();
    let at_least = |k: i64| count.is_some_and(|r| r.lo.is_some_and(|lo| lo >= k));
    let always_present = match head {
        "first" | "last" => provably_non_empty(&collection_ty),
        "second" => at_least(2),
        "third" => at_least(3),
        _ => {
            items.len() >= 3 && {
                match items[2] {
                    Value::Int(k) => k >= 0 && at_least(k + 1),
                    // A local, or any computed index with an interval — `(bit/and i 1)` is
                    // `int[0..1]`; only a local can be bounded by a guard.
                    index_form => {
                        let index = expr_ty(heap, index_form, ctx).and_then(|t| t.int_range());
                        let non_negative = index.is_some_and(|r| r.lo.is_some_and(|lo| lo >= 0));
                        // A guard bounded this index: the bare local (`i < (count xs)`) or the
                        // offset expression a scan writes (`(nth s (+ i 1))` under `(< (+ i 1)
                        // n)`). The fact is keyed on the base local and the offset, and a
                        // larger proved offset covers a smaller one.
                        let bounded_by_guard = match arg {
                            Value::Sym(collection) => super::guards::local_plus_offset(
                                heap, index_form, ctx,
                            )
                            .is_some_and(|(local, offset)| {
                                ctx.is_index_bound_at(local, collection, offset)
                            }),
                            _ => false,
                        };
                        let bounded_by_interval =
                            index.and_then(|r| r.hi).is_some_and(|hi| at_least(hi + 1));
                        non_negative && (bounded_by_guard || bounded_by_interval)
                    }
                }
            }
        }
    };
    Some(if always_present {
        elem
    } else {
        elem.union(nth_default().unwrap_or(Ty::of(Tag::Nil)))
    })
}

/// `(seq/filter coll pred)` keeps `coll`'s element type — `nil | list<A>` (ADR-078) —
/// NARROWED by the predicate when it is a type predicate: `(seq/filter xs int?)` keeps
/// only the items `int?` admits, so the elements are `elem ∩ int` (the `Ty::tested_by`
/// bridge occurrence typing uses for an `if` guard). `filter` builds a LIST whatever it
/// is handed, so with the element type unknown the result is still `list`.
fn filter_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, *items.get(1)?, ctx);
    let elem = collection_ty.as_ref().and_then(|t| t.elem_ty_union());
    let elem = match (elem, items.get(2).and_then(|p| predicate_kept_ty(*p))) {
        (Some(elem), Some(tested)) => Some(elem.intersect(tested)),
        (None, tested @ Some(_)) => tested,
        (elem, None) => elem,
    };
    list_result(elem).or_else(|| collection_ty.map(|_| Ty::LIST))
}

/// `(seq/find coll pred)` is ONE item that passes, or `nil` — the element type (narrowed
/// by a type predicate exactly as `filter`'s is) with `nil` for "none did".
fn find_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let elem = expr_ty(heap, items[1], ctx)?.elem_ty();
    let elem = match (elem, predicate_kept_ty(items[2])) {
        (Some(elem), Some(tested)) => Some(elem.intersect(tested)),
        (None, tested @ Some(_)) => tested,
        (elem, None) => elem,
    };
    elem.map(|elem| elem.union(Ty::of(Tag::Nil)))
}

/// `(seq/reject coll pred)` keeps what `pred` REJECTS: with a type predicate the
/// survivors are `elem ∖ tested`, so `(seq/reject xs nil?)` answers `list<T>` from a
/// `list<nil | T>`.
fn reject_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, items[1], ctx);
    let elem = collection_ty.as_ref().and_then(|t| t.elem_ty_union());
    let elem = match (elem, predicate_tested_ty(items[2])) {
        (Some(elem), Some(tested)) => Some(elem.difference(tested)),
        (elem, _) => elem,
    };
    list_result(elem).or_else(|| collection_ty.map(|_| Ty::LIST))
}

/// Element-preserving reshapers whose sequence is the first argument (data-first,
/// ADR-308): `reverse`, `sort`, `sort-by`, and `%map-pairs` — the kernel's `seq` over a
/// map, whose `[k v]` entries walk a `map<K, V>` as `(tuple K V)`. A non-empty input
/// stays non-empty.
fn same_elements_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, *items.get(1)?, ctx);
    let elem = collection_ty.as_ref().and_then(|t| t.elem_ty_union());
    list_result_over(collection_ty.as_ref(), elem)
}

/// `seq/distinct` / `seq/dedupe` drop duplicates: the same elements, and a non-empty
/// input stays non-empty, but the LENGTH only shrinks — at most the input's, and at
/// least one when the input had one.
fn deduplicated_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, *items.get(1)?, ctx);
    let elem = collection_ty.as_ref().and_then(|t| t.elem_ty_union());
    let shrunk = collection_ty
        .as_ref()
        .and_then(|t| t.count_range())
        .map(|range| {
            let lo = if input_non_empty(collection_ty.as_ref(), range) {
                1
            } else {
                0
            };
            Range::new(Some(lo), range.hi)
        });
    match shrunk {
        Some(range) => Some(list_with_len(elem.unwrap_or(Ty::ANY), range)),
        None => list_result_over(collection_ty.as_ref(), elem),
    }
}

/// `(seq x)` PASSES THROUGH a vector, string, set, list or `nil` — `(seq [1 2])` is the
/// vector `[1 2]` — so over those it is the identity, exactly. A plain map becomes its
/// `[k v]` entries and `bytes` its octets. A RECORD walks its `Seqable` view, which an
/// impl may redefine, and a rope or table only has one if an impl gives it: no claim.
/// Answered kind by kind and unioned, so `nil | vector | bytes` is itself, itself and a
/// list of octets.
fn seq_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, items[1], ctx)?;
    let passes = [Tag::Vector, Tag::Str, Tag::Set, Tag::Pair, Tag::Nil]
        .into_iter()
        .fold(Ty::NEVER, |acc, tag| acc.union(Ty::of(tag)));
    let bytes = Ty::of(Tag::Bytes);
    let map = Ty::of(Tag::Map);
    let handled = passes.clone().union(bytes.clone()).union(map.clone());
    if !collection_ty.clone().difference(handled).is_never() {
        return None;
    }
    let mut out = collection_ty.clone().intersect(passes);
    let bytes_part = collection_ty.clone().intersect(bytes);
    if !bytes_part.is_never() {
        let octets = Ty::int_in(Range::new(Some(0), Some(255)));
        out = out.union(list_result_over(Some(&bytes_part), Some(octets))?);
    }
    let map_part = collection_ty.intersect(map);
    if !map_part.is_never() {
        if may_carry_identity(&map_part) {
            return None;
        }
        let elem = map_part.elem_ty_union();
        out = out.union(list_result_over(Some(&map_part), elem)?);
    }
    Some(out)
}

/// `rest` / `but-last`: the same elements, one fewer.
fn drop_one_ty(heap: &Heap, is_rest: bool, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let collection_ty = expr_ty(heap, *items.get(1)?, ctx);
    // `rest` is the pair's TAIL, whatever that holds — a vector under an improper
    // `(cons 1 [2 3])` — and a `bytes` value's rest is bytes again. Neither is the list
    // the rule below (and the primitive's flat `list` return) claims.
    if is_rest && collection_ty.as_ref().is_some_and(rest_may_not_be_a_list) {
        return Some(Ty::ANY);
    }
    // Of a positional shape, the shape less its head (or its last): exact, and `nil` when
    // there was one element (or none).
    if let Some(elems) = collection_ty.as_ref().and_then(|t| t.positional_elems()) {
        let kept: Vec<Ty> = if elems.is_empty() {
            Vec::new()
        } else if is_rest {
            elems[1..].to_vec()
        } else {
            elems[..elems.len() - 1].to_vec()
        };
        return Some(if kept.is_empty() {
            Ty::of(Tag::Nil)
        } else {
            Ty::list_shape_of(kept)
        });
    }
    // One shorter than the input: `nil` only when the input may have had one element.
    let shorter = collection_ty
        .as_ref()
        .and_then(|t| t.count_range())
        .map(|range| {
            Range::new(
                range.lo.map(|lo| (lo - 1).max(0)),
                range.hi.map(|hi| (hi - 1).max(0)),
            )
        });
    let elem = collection_ty.and_then(|t| t.elem_ty_union());
    match (elem, shorter) {
        (Some(elem), Some(range)) => Some(list_with_len(elem, range)),
        (elem, _) => list_result(elem),
    }
}

/// `(count x)` is the LENGTH interval of `x`'s type (ADR-350): the arity of a positional
/// shape exactly, `0` for `nil`, at least 1 for a `pair`, what a guard narrowed a
/// collection's length to.
fn count_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let arg_ty = expr_ty(heap, items[1], ctx)?;
    // A CLOSED record is its field count: `(count p)` over `(record :x int :y int)` is
    // exactly 2 (an optional field may be absent, hence `[required, declared]`); an OPEN
    // shape declines. Read here rather than from `count_range`, because a length in the
    // type is a refinement every RELATION consults and a bare `map` must still fit a record
    // shape. A `defrecord` value counts through its `Seqable` view instead, so its shape is
    // not its count: `(count (pt 1 2))` is 2, where the shape has 3 keys.
    let range = if may_carry_identity(&arg_ty) {
        arg_ty.count_range()?
    } else {
        arg_ty
            .record_field_count()
            .or_else(|| arg_ty.count_range())?
    };
    Some(Ty::int_in(range))
}

/// `(range …)` is "a range of integers" (its own docstring): every argument an int means
/// every element is one. Empty ranges are `nil`; a bound that is not an int answers
/// nothing.
fn range_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let bounds: Vec<Option<Ty>> = items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
    let all_int = bounds
        .iter()
        .all(|t| t.as_ref().is_some_and(|t| t.is_subtype(&Ty::of(Tag::Int))));
    // The elements carry the bounds' INTERVAL (ADR-350): `(range n)` counts from 0 up
    // to `n - 1`, `(range a b)` from `a` up to `b - 1`, so an element is never below
    // the start and never reaches the end — `(range (count xs))` hands its callback
    // an `int[0..]`, which is the non-negative half of what `(nth xs i)` asks before it
    // reads an element. A bound with no interval leaves that end open.
    let int = {
        let range_of = |t: &Option<Ty>| t.as_ref().and_then(|t| t.int_range());
        let (start, end) = match items.len() {
            2 => (Some(Range::point(0)), range_of(&bounds[0])),
            _ => (range_of(&bounds[0]), range_of(&bounds[1])),
        };
        // An ascending range lies in `[start, end - 1]`; with a STEP (`(range a b s)`)
        // it may descend instead — `(range 5 2 -1)` is `(5 4 3)`, in `[end + 1, start]`.
        // A step whose sign is not known is either, so the hull of both.
        let ascending = Range::new(
            start.and_then(|r| r.lo),
            // Saturating: a bound at the edge of `i64` (`arithmetic_edge_test`'s
            // `(range math/max-int …)`) must not overflow the checker.
            end.and_then(|r| r.hi).map(|h| h.saturating_sub(1)),
        );
        let descending = Range::new(
            end.and_then(|r| r.lo).map(|l| l.saturating_add(1)),
            start.and_then(|r| r.hi),
        );
        let step = bounds.get(2).and_then(range_of);
        let interval = match step {
            None if items.len() == 4 => {
                if ascending.is_empty() {
                    descending
                } else if descending.is_empty() {
                    ascending
                } else {
                    Range::hull(ascending, descending)
                }
            }
            Some(s) if s.lo.is_some_and(|lo| lo > 0) => ascending,
            Some(s) if s.hi.is_some_and(|hi| hi < 0) => descending,
            Some(_) => {
                if ascending.is_empty() {
                    descending
                } else if descending.is_empty() {
                    ascending
                } else {
                    Range::hull(ascending, descending)
                }
            }
            None => ascending,
        };
        if interval.is_empty() {
            // `(range 0)`: no element at all — the list is `nil`
            return Some(Ty::of(Tag::Nil));
        }
        if interval.is_all() {
            Ty::of(Tag::Int)
        } else {
            Ty::int_in(interval)
        }
    };
    // A literal bound proves the range non-empty: `(range 5)`, `(range 2 5)`.
    let non_empty = match (items.get(1), items.get(2), items.len()) {
        (Some(Value::Int(n)), None, 2) => *n > 0,
        (Some(Value::Int(a)), Some(Value::Int(b)), 3) => a < b,
        // …and with a literal step, in the step's direction.
        (Some(Value::Int(a)), Some(Value::Int(b)), 4) => match items.get(3) {
            Some(Value::Int(s)) if *s > 0 => a < b,
            Some(Value::Int(s)) if *s < 0 => a > b,
            _ => false,
        },
        _ => false,
    };
    // `(range n)` has exactly `n` elements when `n ≥ 0` (none otherwise), so its
    // length is `n`'s interval clipped at 0 — and non-empty when `n ≥ 1`.
    let length = match (items.len(), expr_ty(heap, items[1], ctx)) {
        (2, Some(n)) => n
            .int_range()
            .map(|r| Range::new(Some(r.lo.unwrap_or(0).max(0)), r.hi.map(|h| h.max(0)))),
        _ => None,
    };
    // The length is kept either way, so `(nth (into [] (map (range 1000) f)) 999)` is
    // present.
    if all_int && non_empty {
        Some(match length {
            Some(r) => Ty::list_of(int).with_len(r),
            None => Ty::list_of(int),
        })
    } else if all_int {
        match length {
            Some(r) => Some(list_with_len(int, r)),
            None => list_result(Some(int)),
        }
    } else {
        None
    }
}

/// `(vec coll)` — the same elements, as a vector. Unknown elements → a bare vector.
fn vec_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let elem = expr_ty(heap, items[1], ctx).and_then(|t| t.elem_ty_union());
    Some(match elem {
        Some(elem) => Ty::vector_of(elem),
        None => Ty::of(Tag::Vector),
    })
}

/// `(into target coll)` — `target`'s kind, holding `target`'s elements and `coll`'s. Only
/// the kinds whose element story is plain: a vector, list or set target keeps its
/// elements and gains `coll`'s; a map target is that kind, unrefined (its entries come
/// from pairs).
fn into_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let target = expr_ty(heap, items[1], ctx)?;
    let added = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty_union());
    let own = if target.is_subtype(&Ty::of(Tag::Nil)) {
        Some(Ty::NEVER)
    } else {
        target.elem_ty()
    };
    let joined = match (own, added) {
        (Some(a), Some(b)) => Some(a.union(b)),
        _ => None,
    };
    if target.is_subtype(&Ty::of(Tag::Vector)) {
        let out = match joined {
            Some(e) => Ty::vector_of(e),
            None => Ty::of(Tag::Vector),
        };
        // The length is the target's plus the source's, when both are known — so
        // `(nth (into [] (map (range 1000) f)) 999)` is present, as it is.
        let grown = match (
            target.count_range(),
            expr_ty(heap, items[2], ctx).and_then(|t| t.count_range()),
        ) {
            (Some(a), Some(b)) => Some(Range::plus(a, b)),
            _ => None,
        };
        return Some(match grown {
            Some(r) => out.with_len(r),
            None => out,
        });
    }
    if target.is_subtype(&Ty::LIST) {
        // Non-empty if either side is: the target keeps its elements, the source adds.
        let non_empty = provably_non_empty(&target)
            || expr_ty(heap, items[2], ctx).is_some_and(|t| provably_non_empty(&t));
        return Some(match (joined, non_empty) {
            (Some(e), true) => Ty::list_of(e),
            (Some(e), false) => Ty::list_of(e).union(Ty::of(Tag::Nil)),
            (None, _) => Ty::LIST,
        });
    }
    if target.is_subtype(&Ty::of(Tag::Map)) {
        return Some(Ty::of(Tag::Map));
    }
    if target.is_subtype(&Ty::of(Tag::Set)) {
        return Some(match joined {
            Some(e) => Ty::set_of(e),
            None => Ty::of(Tag::Set),
        });
    }
    None
}

/// `(conj coll x …)` — `coll`'s kind, its elements plus the `x`s. A vector, list or set
/// carries the elements; a map is its kind, unrefined. The elements are read over every
/// term of a union (`elem_ty_union`), so a fold accumulator's `(tuple) | vector<string>`
/// — the empty seed beside the grown step — keeps its element type.
fn conj_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let coll = expr_ty(heap, items[1], ctx)?;
    let mut added: Option<Ty> = None;
    for &x in &items[2..] {
        let t = expr_ty(heap, x, ctx)?;
        added = Some(match added {
            Some(a) => a.union(t),
            None => t,
        });
    }
    let added = added?;
    if coll.is_subtype(&Ty::of(Tag::Vector)) {
        let out = match coll.elem_ty_union() {
            Some(e) => Ty::vector_of(e.union(added)),
            None => Ty::of(Tag::Vector),
        };
        let grown = coll
            .count_range()
            .map(|r| Range::plus(r, Range::point(items.len() as i64 - 2)));
        return Some(match grown {
            Some(r) => out.with_len(r),
            None => out,
        });
    }
    if coll.is_subtype(&Ty::of(Tag::Set)) {
        return Some(match coll.elem_ty_union() {
            Some(e) => Ty::set_of(e.union(added)),
            None => Ty::of(Tag::Set),
        });
    }
    if coll.is_subtype(&Ty::LIST) {
        // onto nil: a list of the items; onto a list: that list plus the items
        let own = if coll.is_subtype(&Ty::of(Tag::Nil)) {
            Some(Ty::NEVER)
        } else {
            coll.elem_ty_union()
        };
        return Some(match own {
            Some(e) => Ty::list_of(e.union(added)),
            None => Ty::of(Tag::Pair),
        });
    }
    if coll.is_subtype(&Ty::of(Tag::Map)) {
        return Some(Ty::of(Tag::Map));
    }
    None
}

/// `(merge m …)` — a map; over record shapes, the right-biased union of the shapes.
fn merge_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let tys: Vec<Ty> = items[1..]
        .iter()
        .filter_map(|&a| expr_ty(heap, a, ctx))
        .collect();
    if tys.len() != items.len() - 1 || !tys.iter().all(|t| t.is_subtype(&Ty::of(Tag::Map))) {
        return None;
    }
    // Every argument a record SHAPE: the result is the right-biased union of the
    // shapes — a later field wins outright, so its type and required-ness are the
    // last declaration's — open if any input is open (an undeclared key may then come
    // from that input). Otherwise just a map.
    let shapes: Option<Vec<_>> = tys
        .iter()
        .map(|t| {
            t.record_fields()
                .map(|f| (f.clone(), t.record_is_open().unwrap_or(true)))
        })
        .collect();
    Some(match shapes {
        Some(shapes) => {
            let mut fields = std::collections::BTreeMap::new();
            let mut open = false;
            for (f, o) in shapes {
                open |= o;
                // An OPEN later argument may carry any key it does not declare, with
                // any value — including a key an earlier argument declared, which it
                // then overrides. So every earlier field it does not redeclare keeps
                // its presence (merge never removes a key) but not its type.
                if o {
                    for (k, (ty, _)) in fields.iter_mut() {
                        if !f.contains_key(k) {
                            *ty = Ty::ANY;
                        }
                    }
                }
                for (k, (ty, required)) in f {
                    // An OPTIONAL later field may be absent, and then the earlier
                    // value stands: the union of both, present if either says so.
                    let merged = match fields.get(&k) {
                        Some((earlier, was_required)) if !required => {
                            (earlier.clone().union(ty), *was_required)
                        }
                        _ => (ty, required),
                    };
                    fields.insert(k, merged);
                }
            }
            if open {
                Ty::record_of_open(fields)
            } else {
                Ty::record_of(fields)
            }
        }
        None => Ty::of(Tag::Map),
    })
}

/// `(apply f a b … coll)` — `f` applied to `a`, `b`, … and one more operand of `coll`'s
/// element type, so a callee's type variable binds through the spread as it does in the
/// written-out call (KI-140): `(apply math/max 1 (map xs string/length))` under `(& ?A ->
/// ?A)` is an `int`. The spread's COUNT is unknown, so a ring operator's result is its
/// closure, never one step's interval (`(apply + [1 2])` is an int, not `int[1..2]`). A
/// lambda literal types as its body with its inputs unknown — the flat return.
fn apply_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    match items[1] {
        Value::Sym(f) if !ctx.is_lexical_local(f) => {
            let (spread, coll) = items[2..].split_at(items.len() - 3);
            let mut inputs: Vec<Option<Ty>> =
                spread.iter().map(|&a| expr_ty(heap, a, ctx)).collect();
            inputs.push(expr_ty(heap, coll[0], ctx).and_then(|t| t.elem_ty_union()));
            let ret = callback_ret(heap, items[1], &inputs, ctx)
                .or_else(|| sig_of(heap, f).map(|sig| sig.ret))?;
            Some(if numeric_op_kind(f).is_some() {
                ret.without_int_interval()
            } else {
                ret
            })
        }
        Value::Pair(_) => {
            let n = list_items(heap, items[1])
                .and_then(|l| l.get(1).copied())
                .and_then(|ps| list_items(heap, ps))
                .map(|ps| ps.len())?;
            lambda_ret(heap, items[1], &vec![None; n], ctx)
        }
        _ => None,
    }
}

/// `(cons x xs)` — prepend `x` onto `xs`; the element type is `type(x) | elem(xs)`, both
/// known or the element is unknown. Always a `pair` (never nil), so `list<E>` without the
/// `nil` variant.
fn cons_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let head_ty = expr_ty(heap, items[1], ctx);
    let tail_ty = expr_ty(heap, items[2], ctx);
    // Onto anything but a list (`nil` or a pair) the result is an IMPROPER pair:
    // `(cons 1 [2 3])` is `(1 . [2 3])`, whose `rest` is the vector itself — a bare
    // `pair`.
    if !tail_ty.as_ref().is_some_and(|t| t.is_subtype(&Ty::LIST)) {
        return Some(Ty::of(Tag::Pair));
    }
    // Onto `nil` or onto a positional shape, the result is a shape one longer: the
    // head in front, exactly.
    if let Some(tail) = &tail_ty {
        let tail_shape: Option<Vec<Ty>> = if tail.is_subtype(&Ty::of(Tag::Nil)) {
            Some(Vec::new())
        } else {
            tail.list_shape_elems().cloned()
        };
        if let Some(mut elems) = tail_shape {
            elems.insert(0, head_ty.clone().unwrap_or(Ty::ANY));
            return Some(Ty::list_shape_of(elems));
        }
    }
    // A `nil` tail (`'()`, `nil`) contributes no elements at all, so the list's
    // elements are exactly the head's — `(cons 1 '())` is `list<1>`, not a bare `pair`.
    let tail_elem = tail_ty.clone().and_then(|t| {
        if t.is_subtype(&Ty::of(Tag::Nil)) {
            Some(Ty::NEVER)
        } else {
            t.elem_ty()
        }
    });
    let longer = tail_ty
        .as_ref()
        .and_then(|t| t.count_range())
        .map(|r| Range::plus(r, Range::point(1)));
    match (head_ty, tail_elem) {
        (Some(h), Some(e)) => Some(match longer {
            Some(r) => Ty::list_of(h.union(e)).with_len(r),
            None => Ty::list_of(h.union(e)),
        }),
        _ => Some(Ty::of(Tag::Pair)), // one side unknown → unrefined pair
    }
}

/// `(append xs ys …)` — variadic list concatenation. The element type is the union of
/// every argument's; any argument with an unknown element type falls through to the flat
/// result.
fn append_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    if items.len() == 1 {
        return Some(Ty::of(Tag::Nil)); // (append) = nil
    }
    let mut acc: Option<Ty> = None;
    let mut any_non_empty = false;
    for &arg in &items[1..] {
        let arg_ty = expr_ty(heap, arg, ctx)?;
        any_non_empty |= provably_non_empty(&arg_ty);
        // `nil` — the empty list — contributes no elements at all.
        let elem = if arg_ty.is_subtype(&Ty::of(Tag::Nil)) {
            Ty::NEVER
        } else {
            arg_ty.elem_ty()?
        };
        acc = Some(match acc {
            Some(a) => a.union(elem),
            None => elem,
        });
    }
    // One non-empty argument makes the whole result non-empty.
    if any_non_empty {
        acc.map(Ty::list_of)
    } else {
        list_result(acc)
    }
}

/// `(get m k [default])` → `V | nil` on a `map<K, V>` (nil = key absent and no default).
/// On a record shape with a *literal keyword* key, the declared field type wins. An
/// undeclared or dynamic key falls through: records are open, so an unknown key's type is
/// genuinely unknown, not an error.
fn get_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let map_arg = *items.get(1)?;
    let map_ty = expr_ty(heap, map_arg, ctx);
    // `(get m k default)`: an ABSENT key yields the default, so the absence case reads
    // as the default's type rather than `nil` — `(get dt :hour 0)` is `int`. A default
    // whose type can't be pinned falls back to the two-argument reading (which admits
    // `nil` — a superset, since it can only over-state the absence case).
    let default_ty = match items.get(3) {
        Some(&d) if items.len() == 4 => expr_ty(heap, d, ctx),
        _ => None,
    };
    if let Value::Keyword(key) = items[2] {
        if let Some(d) = default_ty.as_ref()
            && let Some(fty) = map_ty
                .as_ref()
                .and_then(|t| t.record_field_ty_with_default(key, d))
        {
            return Some(fty);
        }
        if let Some(fty) = map_ty.as_ref().and_then(|t| t.record_field_ty(key)) {
            return Some(fty);
        }
    }
    let (_, v) = map_ty.as_ref().and_then(Ty::map_kv)?;
    Some(match default_ty {
        Some(d) => v.clone().union(d),
        None => v.clone().union(Ty::of(Tag::Nil)),
    })
}

/// `(get-in m [k …] [default])` with a LITERAL vector path → the `get` rule applied key
/// by key: a literal keyword reads the record field, any other key reads a `map<K, V>`'s
/// value. The walk STOPS at a key that is absent or whose value is not a map —
/// `%get-in-walk` answers the default there — so every step but the last strips the
/// `nil` its read admits and remembers that the default is among the answers; the last
/// step reads with the default in place of absence, keeping a PRESENT nil as nil. A step
/// neither rule can type leaves the whole read unknown.
fn get_in_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    if let Value::Vector(path_id) = items[2] {
        let path = heap.vector(path_id).to_vec();
        let nil = Ty::of(Tag::Nil);
        let default = match items.get(3) {
            Some(&d) => expr_ty(heap, d, ctx)?,
            None => nil.clone(),
        };
        let mut cur = expr_ty(heap, items[1], ctx)?;
        let mut defaulted = false;
        for (index, key) in path.iter().enumerate() {
            let last = index + 1 == path.len();
            // A step reads only the maps among what reached it; anything else —
            // nil, or a non-map value at an inner key — stops the walk at the default.
            let readable = cur.clone().difference(nil.clone());
            if readable.is_never() {
                return Some(default);
            }
            if !readable.is_subtype(&Ty::of(Tag::Map)) {
                return None;
            }
            let read = match key {
                Value::Keyword(name) if last => {
                    readable.record_field_ty_with_default(*name, &default)
                }
                Value::Keyword(name) => readable.record_field_ty(*name),
                _ => None,
            }
            .or_else(|| {
                readable
                    .map_kv()
                    .map(|(_, v)| v.union(if last { default.clone() } else { nil.clone() }))
            })?;
            if !last && read.contains_tag(Tag::Nil) {
                defaulted = true;
            }
            cur = read;
        }
        return Some(if defaulted { cur.union(default) } else { cur });
    }
    None
}

/// `(keys m)` → `nil | list<K>`. On a **closed** record shape the keys are exactly the
/// declared field names, so the element type is that literal set. An OPEN record may
/// carry keys nothing declares, so it falls through to the `map<K, V>` rule.
fn keys_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let map_arg = *items.get(1)?;
    let map_ty = expr_ty(heap, map_arg, ctx);
    // A record's keys come from its `Lookup`/`Seqable` impls (id-free by default): no
    // claim from the raw shape.
    if map_ty.as_ref().is_some_and(may_carry_identity) {
        return None;
    }
    if let Some(shape) = closed_record_fields(map_ty.as_ref()) {
        let names = shape
            .keys()
            .fold(Ty::NEVER, |acc, name| acc.union(Ty::keyword_lit(*name)));
        // An optional field may be absent, so the union over ALL declared names is
        // an over-approximation of the keys actually present — sound, and the only
        // answer available without knowing the value.
        return list_result(Some(names));
    }
    let (key, _) = map_ty.as_ref().and_then(Ty::map_kv)?;
    list_result(Some(key.clone()))
}

/// `(vals m)` → `nil | list<V>`. On a closed record, every value present is one of the
/// declared field types.
fn vals_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let map_arg = *items.get(1)?;
    let map_ty = expr_ty(heap, map_arg, ctx);
    if map_ty.as_ref().is_some_and(may_carry_identity) {
        return None;
    }
    if let Some(shape) = closed_record_fields(map_ty.as_ref()) {
        let types = shape
            .values()
            .fold(Ty::NEVER, |acc, (ty, _required)| acc.union(ty.clone()));
        return list_result(Some(types));
    }
    let (_, value_ty) = map_ty.as_ref().and_then(Ty::map_kv)?;
    list_result(Some(value_ty.clone()))
}

/// `(dissoc m k …)` → the same record shape without those fields, with literal-keyword
/// keys. OPEN records too: "open" means *at least* these fields, so removing a declared
/// one drops it and removing an undeclared one leaves the shape alone — either way the
/// remaining fields are still all present.
fn dissoc_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let map_ty = expr_ty(heap, *items.get(1)?, ctx);
    let shape = record_shape_of(map_ty.as_ref())?;
    let removed = literal_keyword_args(&items[2..])?;
    let mut fields = shape.fields;
    for name in removed {
        fields.remove(&name);
    }
    Some(if shape.open {
        Ty::record_of_open(fields)
    } else {
        Ty::record_of(fields)
    })
}

/// `(update r :k f …)`, `(assoc-in r [:k …] v)`, `(update-in r [:k …] f …)` — the
/// record-keeping siblings of `assoc`. Each replaces ONE top-level key, named by a literal
/// keyword (the first path element for the `-in` forms), with a value the checker does
/// not compute: the shape survives with that field unknown, and the other fields keep
/// their types. On a receiver with no shape and a keyword key the answer is `map`, as for
/// `assoc`, so a model threaded through `update` keeps being a map.
fn update_ty(heap: &Heap, is_update: bool, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let key = if is_update {
        match items[2] {
            Value::Keyword(name) => Some(name),
            _ => None,
        }
    } else {
        match items[2] {
            Value::Vector(id) => match heap.vector(id).first() {
                Some(Value::Keyword(name)) => Some(*name),
                _ => None,
            },
            _ => None,
        }
    };
    let map_ty = expr_ty(heap, items[1], ctx);
    if let Some(key) = key {
        if let Some(shape) = record_shape_of(map_ty.as_ref()) {
            let mut fields = shape.fields;
            fields.insert(key, (Ty::ANY, true));
            return Some(if shape.open {
                Ty::record_of_open(fields)
            } else {
                Ty::record_of(fields)
            });
        }
        if map_ty.as_ref().and_then(Ty::map_kv).is_none() {
            return Some(Ty::of(Tag::Map));
        }
    } else if map_ty
        .as_ref()
        .is_some_and(|t| t.is_subtype(&Ty::of(Tag::Map)))
    {
        // A DYNAMIC key on a known map — `(update m (:key spec) …)`. Which field
        // changed is unknown, so the shape cannot be kept; but the result is a map,
        // never the `vector | map` of `update`'s body.
        return Some(Ty::of(Tag::Map));
    }
    // `assoc-in` / `update-in` keep a VECTOR node a vector (updated by index) and turn
    // any other node into a map. So a receiver that cannot be a vector answers `map`,
    // exactly. A receiver of UNKNOWN type answers unknown: the body's `vector | map`
    // would be two guesses about a value nobody typed. A receiver known to include a
    // vector keeps the body's union, which is the honest answer there.
    if !is_update {
        let vector = Ty::of(Tag::Vector);
        match map_ty.as_ref() {
            Some(t) if t.is_disjoint(&vector) => return Some(Ty::of(Tag::Map)),
            None => return Some(Ty::ANY),
            Some(t) if *t == Ty::ANY => return Some(Ty::ANY),
            Some(_) => {}
        }
    }
    None
}

/// `(assoc m k1 v1 …)` → `map<K, V>` with the assoc'd keys and values UNIONED into the
/// refinement: `(assoc m :extra "text")` on a `(map keyword int)` holds a string at
/// `:extra`. A key or value whose own type is unknown widens that side to `any`.
fn assoc_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let map_arg = *items.get(1)?;
    let map_ty = expr_ty(heap, map_arg, ctx);
    // Over a UNION of record shapes, with literal-keyword keys: the update applied to
    // each alternative, and the alternatives kept — a fold accumulator after one step
    // (`{col: int, ops: (tuple)} | {col: int, ops: vector<string>}`) keeps its fields.
    if let Some(terms) = map_ty
        .as_ref()
        .filter(|t| t.alts.is_some())
        .map(Ty::terms_vec)
        && let Some(updates) = literal_keyword_pairs(&items[2..])
    {
        let values: Vec<(value::Symbol, Ty)> = updates
            .into_iter()
            .map(|(name, form)| (name, expr_ty(heap, form, ctx).unwrap_or(Ty::ANY)))
            .collect();
        let updated = terms.iter().try_fold(Ty::NEVER, |out, term| {
            let shape = record_shape_of(Some(term))?;
            let mut fields = shape.fields;
            for (name, vty) in &values {
                fields.insert(*name, (vty.clone(), true));
            }
            Some(out.union(if shape.open {
                Ty::record_of_open(fields)
            } else {
                Ty::record_of(fields)
            }))
        });
        // A term that is not a record shape leaves this to the rules below.
        if let Some(updated) = updated {
            return Some(updated);
        }
    }
    // On a record shape with literal-keyword keys, carry the SHAPE forward with the
    // assoc'd fields added or replaced — required, since `assoc` definitely puts them
    // there. Without this a closed record degrades to a flat `map` on the first
    // update, which would make closed records (ADR-264) unusable in the idiom that
    // builds one field at a time. An unknown value type contributes `any` rather
    // than dropping the whole shape.
    if let Some(shape) = record_shape_of(map_ty.as_ref()) {
        if let Some(updates) = literal_keyword_pairs(&items[2..]) {
            let mut fields = shape.fields;
            for (name, value_form) in updates {
                let vty = expr_ty(heap, value_form, ctx).unwrap_or(Ty::ANY);
                fields.insert(name, (vty, true));
            }
            return Some(if shape.open {
                Ty::record_of_open(fields)
            } else {
                Ty::record_of(fields)
            });
        }
        // A CLOSED shape under a computed key — `(assoc {} k v)` in a table-building
        // fold — is a `map<K, V>`: the declared fields' keys and values, plus the
        // assoc'd ones. Sound for what a map type claims (every key a `K`, every
        // value a `V`); an open shape keeps the flat `map` below, since its undeclared
        // keys could be anything. This is what lets a fold from `{}` settle on
        // `map<int, int>` instead of a bare `map` whose every `get` is `any`.
        if !shape.open {
            let mut key_ty = Ty::NEVER;
            let mut val_ty = Ty::NEVER;
            for (name, (fty, _)) in &shape.fields {
                key_ty = key_ty.union(Ty::keyword_lit(*name));
                val_ty = val_ty.union(fty.clone());
            }
            for pair in items[2..].as_chunks::<2>().0 {
                key_ty = key_ty.union(expr_ty(heap, pair[0], ctx).unwrap_or(Ty::ANY));
                val_ty = val_ty.union(expr_ty(heap, pair[1], ctx).unwrap_or(Ty::ANY));
            }
            return Some(Ty::map_of(key_ty, val_ty));
        }
    }
    if let Some((k, v)) = map_ty.as_ref().and_then(Ty::map_kv) {
        let mut key_ty = k.clone();
        let mut val_ty = v.clone();
        for pair in items[2..].as_chunks::<2>().0 {
            key_ty = key_ty.union(expr_ty(heap, pair[0], ctx).unwrap_or(Ty::ANY));
            val_ty = val_ty.union(expr_ty(heap, pair[1], ctx).unwrap_or(Ty::ANY));
        }
        return Some(Ty::map_of(key_ty, val_ty));
    }
    // No usable receiver type but every key a literal keyword: the result is a `map`.
    // `assoc`'s body is `(if (vector? coll) (%vector-assoc …) (%map-assoc …))`, but
    // `%vector-assoc` RAISES on a keyword index, so a keyword-keyed call that returns
    // at all returns a map.
    if literal_keyword_pairs(&items[2..]).is_some()
        || map_ty
            .as_ref()
            .is_some_and(|t| t.is_subtype(&Ty::of(Tag::Map)))
    {
        return Some(Ty::of(Tag::Map));
    }
    None
}

/// `(string/->number "1")` is `1`, not `number | failure`: the declared signature admits
/// a failure because most calls can fail, but a call whose argument is a known string
/// cannot (ADR-310: the failure belongs where it can happen). Keyed on the argument's
/// TYPE, so a narrowed variable is covered too, and a union of literals answers the union
/// of their results. Parseability is decided by `classify_numeric_text`, the function the
/// runtime calls, so the type cannot contradict the value.
fn parse_number_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let literals: Vec<_> = expr_ty(heap, items[1], ctx)
        .as_ref()
        .and_then(Ty::as_lit_str)
        .map(|set| set.iter().cloned().collect())?;
    // A radix, if present, must itself be a literal in range; otherwise decline.
    let radix = match items.get(2) {
        None => None,
        Some(&Value::Int(radix)) if (2..=36).contains(&radix) => Some(radix as u32),
        Some(_) => return None,
    };
    literals
        .iter()
        .map(
            |literal| match crate::builtins::classify_numeric_text(literal, radix) {
                crate::builtins::NumericText::Int(n) => Ty::int_lit(n),
                // A bignum's tag is not `int`, and a float is not either; name the honest
                // widening rather than guess a tag.
                crate::builtins::NumericText::Big => Ty::NUMBER,
                crate::builtins::NumericText::Float => Ty::of(Tag::Float),
                crate::builtins::NumericText::NotANumber => Ty::of(Tag::Failure),
            },
        )
        .reduce(Ty::union)
}

/// `(map coll f)` → `nil | list<B>`, `B` = the callback's return type applied to `coll`'s
/// element type. Unknown callback / element → flat `list`.
fn map_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let (collection, callback) = super::sigs::combinator_args(items)?;
    let collection_ty = expr_ty(heap, collection, ctx);
    let elem = collection_ty.as_ref().and_then(|t| t.elem_ty_union());
    let mapped = callback_ret(heap, callback, &[elem], ctx);
    list_result_over(collection_ty.as_ref(), mapped)
}

/// `(seq/keep coll f)` — `map` then drop the `nil` results: `nil | list<B>` for `B` the
/// callback's return with `nil` REMOVED. Exact, not a guess: `keep`'s body is `(if (nil?
/// y) acc (cons y acc))`, so nil is the one thing that cannot reach the result.
fn keep_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let (collection, callback) = super::sigs::combinator_args(items)?;
    let elem = expr_ty(heap, collection, ctx).and_then(|t| t.elem_ty_union());
    let kept = callback_ret(heap, callback, &[elem], ctx).map(|t| {
        let non_nil = t.clone().difference(Ty::of(Tag::Nil));
        // A callback that only ever yields nil keeps an empty list, i.e. `nil`; `list<never>`
        // would be true but useless downstream, so keep the unstripped type there.
        if non_nil.is_never() { t } else { non_nil }
    });
    list_result(kept)
}

/// `(seq/interpose coll sep)` — weave `sep` between `coll`'s elements: `nil | list<A |
/// type(sep)>`. Both must be known, else flat.
fn interpose_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let elem = expr_ty(heap, items[1], ctx).and_then(|t| t.elem_ty_union())?;
    let separator_ty = expr_ty(heap, items[2], ctx)?;
    list_result(Some(separator_ty.union(elem)))
}

/// `(reduce coll init f)` / `(fold coll init f)` reduce to an accumulator typed `ty(init)
/// | B`, where `B` is the 2-arg callback's return. The result joins the empty-input case
/// (`init`) with a step result (`B`). The no-init `(reduce coll f)` starts the
/// accumulator at `coll`'s first element. Both `init` and `B` must be known, else flat.
/// `(%record :id {fields…})` — what a `defrecord` constructor's body is (ADR-542): the
/// record shape the checker has always modelled a record by, the field literal's shape plus
/// the `__id__` pseudo-field holding the id literal. The runtime value is its own kind; the
/// checker keeps this map-shaped model until it moves to a `Record` tag (ADR-542, step 3),
/// so a constructor's body agrees with the record type its `sig` declares.
fn record_ctor_ty(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let Value::Keyword(id) = items[1] else {
        return None;
    };
    let mut fields = expr_ty(heap, items[2], ctx)?.record_fields()?.clone();
    fields.insert(
        value::intern(crate::core::keywords::RECORD_ID),
        (Ty::keyword_lit(id), true),
    );
    Some(Ty::record_of(fields))
}

fn reduce_ty(heap: &Heap, is_reduce: bool, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let (collection, f) = super::sigs::combinator_args(items)?;
    let collection_ty = expr_ty(heap, collection, ctx);
    let elem = collection_ty.as_ref().and_then(|t| t.elem_ty_union());
    let init_ty = match items.len() {
        // (fold coll init f) / (reduce coll init f)
        4 => expr_ty(heap, items[2], ctx),
        // (reduce coll f) — the initial accumulator is the first element
        3 if is_reduce => elem.clone(),
        _ => return None,
    };
    // Over a PROVABLY non-empty sequence the step runs at least once, so the result is a
    // STEP result and the empty-input case (`init`) does not join in. A no-init `(reduce
    // coll f)` takes its first element as the accumulator and steps over the REST, so its
    // step runs only over a collection of two or more: `(reduce [1] +)` is `1`, the seed.
    let length = collection_ty.as_ref().and_then(|t| t.count_range());
    let first_step_at = if items.len() == 3 { 2 } else { 1 };
    let ran_at_least_once = match items.len() {
        3 => length
            .and_then(|r| r.lo)
            .is_some_and(|lo| lo >= first_step_at),
        _ => collection_ty.as_ref().is_some_and(provably_non_empty),
    };
    // …and never, over a collection too short for a step: the result is the seed.
    if length
        .and_then(|r| r.hi)
        .is_some_and(|hi| hi < first_step_at)
    {
        return init_ty;
    }
    // A numeric operator folded over a numeric sequence stays inside the operator's
    // closure: by induction the accumulator is `init` at first and `(op acc x)` after,
    // so with `init` and every element in a closed set the result is in it too —
    // `(reduce + 0 ints)` is an `int`, not the `number | map` the accumulator-as-`any`
    // path below produces from `+`'s declared signature.
    if let (Value::Sym(operator), Some(init), Some(element)) = (f, &init_ty, &elem)
        && let Some(t) = numeric_result(operator, &[init.clone(), element.clone()])
    {
        // The closure class; one step's interval is not the fold's (ADR-350).
        return Some(t.without_int_interval());
    }
    // Seed the accumulator from `init` and take one step to a fixpoint: with `acc₁ =
    // init ∪ f(init, e)`, if `f(acc₁, e) ⊆ acc₁` then by induction every iterate is in
    // `acc₁` — `(fold (fn (h c) (bit/xor (* h 31) …)) 5381 s)` is an int, where seeding
    // `h` as `any` read `(* h 31)` as `number`. Not stable → the `any`-seeded reading
    // (sound: the accumulator is over-approximated).
    // An unknown element is `any` here — still sound, and the accumulator's own closure
    // (`(fn (m s) (math/max m (string/length s)))` over an untyped `lines`) is what
    // matters.
    // A FEW steps, not one: a tuple accumulator whose slots feed each other stabilises
    // one step later than a scalar — `[0 '()]` under `[(inc j) (cons j acc)]` goes
    // `(tuple 0 nil)` → `(tuple int list<0>)` → `(tuple int list<int>)`. Bounded: the
    // union can only grow, a `Ty` is bounded in size (KI-13), and four steps covers a
    // slot feeding a slot feeding a slot.
    if let Some(init) = &init_ty {
        let element = elem.clone().unwrap_or(Ty::ANY);
        let mut acc = init.clone();
        for _ in 0..4 {
            let Some(step) =
                callback_ret(heap, f, &[Some(acc.clone()), Some(element.clone())], ctx)
            else {
                break;
            };
            if step.is_subtype(&acc) {
                return Some(if ran_at_least_once { step } else { acc });
            }
            // An interval that moved goes to its infinity (ADR-350): a counter slot
            // `(inc j)` would otherwise climb `0`, `0 | 1`, … past the step bound.
            acc = acc.clone().union(step).widen_intervals_against(&acc);
        }
    }
    let step = callback_ret(heap, f, &[Some(Ty::ANY), elem], ctx);
    match (init_ty, step) {
        (Some(_), Some(step)) if ran_at_least_once => Some(step),
        (Some(init), Some(step)) => Some(init.union(step)),
        _ => None,
    }
}

/// The type a bare type-predicate argument asserts of the items it keeps — `int?` → `int`.
///
/// Only a bare SYMBOL naming a predicate counts. An inline `(fn (x) …)` or a composed
/// predicate is left alone: nothing here proves what it admits, and guessing would narrow a
/// type on no evidence, which is worse than the wide answer.
fn predicate_tested_ty(pred: Value) -> Option<Ty> {
    let sym = match pred {
        Value::Sym(s) => s,
        _ => return None,
    };
    Ty::tested_by(&value::symbol_name(sym))
}

/// The same for the items a predicate KEEPS (`filter`, `find`): a one-sided predicate
/// (`Ty::implied_by` — `record?` proves `map`) narrows what passes it, where it must not
/// narrow what `reject` drops.
fn predicate_kept_ty(pred: Value) -> Option<Ty> {
    predicate_tested_ty(pred).or_else(|| match pred {
        Value::Sym(s) => Ty::implied_by(&value::symbol_name(s)),
        _ => None,
    })
}

/// The result type of a list-producing combinator (`map`/`filter`): `nil | list<elem>` —
/// empty input maps/filters to `nil`. `None` element → `None`, so the caller falls back
/// to the flat declared `list` (never a too-narrow result).
fn list_result(elem: Option<Ty>) -> Option<Ty> {
    elem.map(|e| Ty::list_of(e).union(Ty::of(Tag::Nil)))
}

/// Is `t` a provably NON-EMPTY sequence — a `list<T>` (the `pair` tag alone; the empty list
/// is `nil`)? The one length fact the lattice states, and the source of every "no `nil`
/// here" tightening below. A vector or set may be empty whatever its element type.
fn provably_non_empty(t: &Ty) -> bool {
    if t.is_subtype(&Ty::of(Tag::Pair)) {
        return true;
    }
    // A LENGTH of at least one, however it was established (ADR-350).
    if t.count_range()
        .is_some_and(|r| r.lo.is_some_and(|lo| lo >= 1))
    {
        return true;
    }
    // A tuple states its arity, and a CLOSED record states its keys — so either can carry
    // the same "has a first element" fact a `list<T>` does. (An open record cannot: it says
    // nothing about a value that declares no field at all. An optional field cannot either
    // — it may be absent, which is the empty case.)
    //
    // Each needs the type to be ONLY that collection, because `nil` is the EMPTY case:
    // `nil | {a: int}` carries a closed record shape and a required field, and reading the
    // shape alone made `(first (if p {:a 1} nil))` answer `(tuple :a, 1)`, dropping the
    // `nil` every caller has to handle. The RECORD gate is the one that fires; the vector
    // one is defensive, since a union with nil currently widens a tuple shape away
    // (`a_union_with_nil_widens_a_tuple_shape_away` pins that, so the day it stops the
    // gate is already there and the pin says why).
    if t.is_subtype(&Ty::of(Tag::Vector))
        && let Some(elems) = t.tuple_elems()
    {
        return !elems.is_empty();
    }
    // (A `defrecord` value walks its `Seqable` view, not its keys — see `may_carry_identity`.)
    if t.is_subtype(&Ty::of(Tag::Map))
        && t.record_is_open() == Some(false)
        && !may_carry_identity(t)
    {
        return t
            .record_fields()
            .is_some_and(|fields| fields.values().any(|(_ty, required)| *required));
    }
    false
}

/// [`list_result`] for a combinator that keeps its input's LENGTH class — `map`, `sort`,
/// `reverse`, `distinct`: a provably non-empty input gives a provably non-empty output, so
/// the `nil` case is dropped. `(map inc '(1 2))` is `list<int>`, not `nil | list<int>`. An
/// input that may be empty keeps the `nil`.
fn list_result_over(input: Option<&Ty>, elem: Option<Ty>) -> Option<Ty> {
    if let Some(r) = input.and_then(|t| t.count_range()) {
        // A closed record's required fields prove non-emptiness where the length
        // interval alone does not (a map's length is unrefined).
        let r = if input.is_some_and(provably_non_empty) {
            Range::meet(r, Range::at_least(1)).unwrap_or(r)
        } else {
            r
        };
        // The length is a fact about the INPUT, so it holds with an unknown element too:
        // `(map (range 1000) mk)` over an untyped `mk` is 1000 of something.
        return Some(list_with_len(elem.unwrap_or(Ty::ANY), r));
    }
    if input.is_some_and(provably_non_empty) {
        elem.map(Ty::list_of)
    } else {
        list_result(elem)
    }
}

/// Is a collection of type `input`, whose length lies in `r`, provably non-empty?
fn input_non_empty(input: Option<&Ty>, r: Range) -> bool {
    r.lo.is_some_and(|lo| lo >= 1) || input.is_some_and(provably_non_empty)
}

/// `list<elem>` of a length within `r` (ADR-350): the `pair` member carries the length
/// (at least 1 there), and `nil` is in the union exactly when `r` admits 0.
fn list_with_len(elem: Ty, r: Range) -> Ty {
    let non_empty = Ty::list_of(elem).with_len(r);
    if Range::subset(Range::point(0), r) {
        non_empty.union(Ty::of(Tag::Nil))
    } else {
        non_empty
    }
}

/// The return type of a HOF callback `f` whose parameters receive the given
/// `inputs` types (`[elem]` for `map`'s `(f x)`; `[any, elem]` for `reduce`/`fold`'s
/// `(f acc x)`, the accumulator over-approximated as `any`). A `None` input is an
/// unknown parameter type.
/// - a named **global** fn → its signature's return type (`sig_of`);
/// - a straight-line lambda with exactly `inputs.len()` plain params → `body`'s
///   type with each `pᵢ` bound to `inputs[i]` (identity preserves its input);
/// - anything else (a local var, an unknown form) → `None` (flat result).
///
/// The lambda case is the only new inference, and it only computes a *forward*
/// result type — it never *checks* the body, so it doesn't reopen the deferred
/// guarded-use false-positive class.
pub(super) fn callback_ret(heap: &Heap, f: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    match f {
        // A LEXICAL local (a `let`/`fn` binding) shadows the global table. Its return type is
        // known exactly when its TYPE is an arrow — a parameter declared `(int -> string)`,
        // or a `let`-bound literal typed from its callers (ADR-355, whose result is the
        // body under the union of what every caller hands it, a superset of this call's) —
        // and `(map xs f)` then answers `list<R>` instead of a bare `list`. A file global is
        // a local too in `is_local`'s sense, but it is exactly the callee the same-file
        // tables below describe, so the guard is the lexical one.
        Value::Sym(s) if ctx.is_lexical_local(s) => ctx
            .get(s)
            .as_ref()
            .and_then(Ty::as_arrow)
            .map(|arrow| arrow.ret.clone())
            .filter(|ret| !ret.is_any()),
        Value::Sym(s) => {
            // A numeric operator as the callback — `(map inc xs)`, `(reduce + 0 xs)` — with
            // every input known: the same closure rules a direct call gets, instead of the
            // operator's declared signature (widened to `number | map` for `Num` records).
            if let Some(tys) = inputs.iter().cloned().collect::<Option<Vec<Ty>>>()
                && let Some(t) = numeric_result(s, &tys)
            {
                return Some(t);
            }
            // A flat `any` answer is where the callback's own body says nothing about its
            // result FROM ITS OWN PARAMS — `(defn step (acc v) (conj acc …))` reads `any`
            // because `acc` is unconstrained. The combinator knows what it hands over, so
            // re-type the body under `inputs` (call-site specialization) before settling.
            let specialize = |flat: Option<Ty>| -> Option<Ty> {
                match flat {
                    Some(t) if !t.is_unrefined_collection() => Some(t),
                    flat => meet_flat(super::sigs::specialized_ret(heap, s, inputs, ctx), flat),
                }
            };
            // The same-file tables first: the file being checked isn't loaded, so the heap
            // knows nothing of its declarations and inferences — and for a name this file
            // redefines, the heap's binding is the OLD one.
            if !ctx.is_lexical_local(s) {
                if let Some(sv) = ctx.declared_sig_with_vars(s) {
                    return Some(sv.resolve_ret(inputs));
                }
                if let Some(sigs) = ctx.declared_overload(s) {
                    return Some(resolve_overload_ret(sigs, inputs));
                }
                if let Some(sg) = ctx.declared_sig(s) {
                    return Some(sg.ret);
                }
                if let Some(sigs) = ctx.inferred_overload(s) {
                    return specialize(Some(resolve_clause_overload_ret(&sigs, inputs)));
                }
                if let Some(sg) = ctx.inferred_fn_sig(s) {
                    return specialize(Some(sg.ret));
                }
            }
            if ctx.is_file_global(s) {
                return specialize(None);
            }
            // A type-variable declaration — a loaded module's, or one the stdlib image
            // carries for a module this check never loaded (ADR-370) — resolves from
            // `inputs` exactly as it does in call position: `(reduce xs math/max)` is the
            // element type, not the body re-typed (and there is no body to re-type for an
            // unloaded module).
            if let Some(sv) = super::sigs::declared_heap_sig_with_vars(heap, s)
                .or_else(|| super::sigs::image_heap_sig_with_vars(heap, s))
            {
                return Some(sv.resolve_ret(inputs));
            }
            // An overloaded callback (ADR-116) — resolve per matching arm from
            // `inputs` instead of a single flat `ret`, same as the call-form case.
            if let Some(sigs) = declared_heap_overload(heap, s) {
                return Some(resolve_overload_ret(&sigs, inputs));
            }
            // …and a declared single arrow that says anything is the contract, not re-typed
            // (ADR-259); a declared `any` is read as no declaration.
            if let Some(sg) = super::sigs::declared_heap_sig(heap, s)
                && !sg.ret.is_any()
            {
                return Some(sg.ret);
            }
            specialize(sig_of(heap, s).map(|sig| sig.ret))
        }
        // A single-clause plain-param literal types directly; a PATTERN-clause literal
        // gets the same clause machinery a named multi-clause defn does.
        Value::Pair(_) => lambda_ret(heap, f, inputs, ctx)
            .or_else(|| super::sigs::clause_lambda_ret(heap, f, inputs, ctx)),
        // A keyword as the callback — `(map ps :x)` — is the accessor `(:x p)` per element,
        // and types exactly as that call does: the field's type on a record shape, `V | nil`
        // on a `map<K, V>`. Anything else (an element with no shape) stays unknown, as the
        // accessor does.
        Value::Keyword(key) => match inputs {
            [Some(elem)] => elem.record_field_ty(key).or_else(|| {
                elem.map_kv()
                    .map(|(_, v)| v.clone().union(Ty::of(Tag::Nil)))
            }),
            _ => None,
        },
        _ => None,
    }
}

/// The type of one quoted datum: a nested list recurses (`'((1) (2))` is `(list (list 1) (list 2))`),
/// an empty list is `nil`, anything else is its `of_value` singleton/tag.
fn quoted_datum_ty(heap: &Heap, d: Value) -> Ty {
    // Deep-form stack safety: recurses into itself, never back through `expr_ty`'s
    // cap, so a deep quoted datum (a macro-emitted template) would overflow otherwise.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || quoted_datum_ty_inner(heap, d))
}

fn quoted_datum_ty_inner(heap: &Heap, d: Value) -> Ty {
    match d {
        Value::Pair(_) => match list_items(heap, d) {
            Some(elems) if !elems.is_empty() => {
                Ty::list_shape_of(elems.iter().map(|&e| quoted_datum_ty(heap, e)).collect())
            }
            Some(_) => Ty::of(Tag::Nil),
            None => Ty::of(Tag::Pair),
        },
        other => Ty::of_value(other),
    }
}

/// Is `form` a `(catch e handler…)` clause — the tail of a surface `try`?
fn is_catch_clause(heap: &Heap, form: Value) -> bool {
    list_items(heap, form)
        .and_then(|l| l.first().copied())
        .is_some_and(|h| matches!(h, Value::Sym(s) if value::symbol_is(s, kw::CATCH)))
}

/// The arrow type of a **simple** single-clause lambda literal `(fn (p…) body)`: one
/// `any` parameter per plain-symbol param (a literal declares no domain) and the body's
/// type as the result. `None` for anything [`lambda_ret`] declines, and when the body's
/// type is unknown — see the caller for why an unknown result must not become `any`.
fn lambda_arrow(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let params = list_items(heap, items[1])?;
    let plain = params.iter().all(|p| match p {
        Value::Sym(s) => !value::symbol_name_ref(*s).starts_with('&'),
        _ => false,
    });
    if !plain {
        return None;
    }
    let inputs: Vec<Option<Ty>> = vec![None; params.len()];
    let ret = lambda_ret(heap, form, &inputs, ctx)?;
    Some(Ty::arrow(Sig::new(vec![Ty::ANY; params.len()], ret)))
}

/// The return type of a **simple** single-clause lambda `(fn (p…) body)` —
/// exactly `inputs.len()` plain-symbol parameters and one body expression —
/// computed by binding each `pᵢ` to `inputs[i]` and typing `body`. `None` for
/// anything subtler (wrong param count / multi-body / docstring / variadic /
/// destructuring / non-`fn` head), so the result stays flat.
fn lambda_ret(heap: &Heap, form: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    let items = list_items(heap, form)?;
    let Some(Value::Sym(head)) = items.first().copied() else {
        return None;
    };
    if !is_fn_head(head) {
        return None;
    }
    // `(fn <param-list> body…)` — a param list and at least one body form. The RESULT is
    // the last form's type; earlier forms run for effect and contribute nothing to it, so
    // typing only the last is exactly as sound as typing a single-form body. (A docstring
    // is just an earlier form here.) `try`'s expansion needs this:
    // `(%try (fn () a b) …)` has a two-form thunk.
    let parts = &items[1..];
    if parts.len() < 2 {
        return None;
    }
    let params = list_items(heap, parts[0])?;
    if params.len() != inputs.len() {
        return None; // arity must match what the combinator supplies
    }
    let mut sub = ctx.clone();
    for (param, input) in params.iter().zip(inputs) {
        let Value::Sym(p) = param else {
            return None; // not a plain-symbol parameter
        };
        // `&`/`&optional`/`&rest` are markers, not parameters: a variadic literal's
        // frame fill differs per call, and binding the marker itself positionally
        // typed `(fn (& args) …)` as a 2-param fn whenever a combinator supplied two
        // inputs — which is exactly the LOWERED pattern-clause lambda's shape, so the
        // bogus answer also masked `clause_lambda_ret`'s recovery below it.
        if value::symbol_name_ref(*p).starts_with('&') {
            return None;
        }
        sub = sub.bind(*p, input.clone());
    }
    expr_ty(heap, *parts.last()?, &sub)
}

/// The call-site answer when a body was re-typed under the call's argument types
/// (`specialized`) and a flat answer already stood (`flat`): their INTERSECTION when both
/// exist. Each is a sound over-approximation of what the call returns, so the meet is
/// one too, and either side can be the sharper: a curated `assoc` says `vector | map`
/// where its prelude body under `any` inputs says `any`; a recursive list builder's body
/// under `list<int>` says `nil | list<int>` where its flat answer says `nil | list`.
/// Taking the specialized answer alone would lose the first to `any`.
fn meet_flat(specialized: Option<Ty>, flat: Option<Ty>) -> Option<Ty> {
    match (specialized, flat) {
        (Some(s), Some(f)) => Some(s.intersect(f)),
        (s, f) => s.or(f),
    }
}

/// Can `(rest x)` over `t` be something other than a list? Yes when `t` admits `bytes`
/// (whose rest is bytes) or a pair nothing proves PROPER — a bare `pair`, which `cons`
/// onto a vector builds and whose rest is that vector. A `list<T>` (an element
/// refinement or a list shape) is proper: every tail of it is `nil` or a `list<T>`.
fn rest_may_not_be_a_list(t: &Ty) -> bool {
    t.terms_vec().iter().any(|term| {
        term.contains_tag(Tag::Bytes)
            || (term.contains_tag(Tag::Pair)
                && term.elem_ty().is_none()
                && term.list_shape_elems().is_none())
    })
}

/// Does this map type declare the `:__id__` a `defrecord` value carries? Such a value is
/// counted, keyed and walked through its `Seqable` view (`count`, `keys`, `vals` and `seq`
/// all dispatch on `:__id__`), which hides the identity key by default and which an impl
/// may redefine — so no rule may read its count or entries off the raw shape.
fn may_carry_identity(t: &Ty) -> bool {
    let id = value::intern("__id__");
    t.terms_vec().iter().any(|term| {
        term.record_fields()
            .and_then(|fields| fields.get(&id))
            .is_some_and(|(ty, _)| !ty.is_subtype(&Ty::of(Tag::Nil)))
    })
}
