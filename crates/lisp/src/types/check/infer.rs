//! Type inference over expressions — expr_ty + result-typing helpers
//! (extracted from guards.rs, file-organization split).
use super::ctx::{resolve_overload_ret, Ctx};
use super::guards::path_of;
use super::sigs::{declared_heap_overload, sig_of};
use super::walk::{is_fn_head, list_items};
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::Ty;
use std::cell::Cell;

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
    // when declared with a plain `def` and rebound via `def` — e.g. `*project-root*`
    // (`(def *project-root* nil)` at load, reassigned to the real path at runtime).
    // The type philosophy makes a redefinable global `dynamic()`, so pinning it to
    // its load-time default would false-positive on every use once it is reassigned
    // (`(path-join *project-root* …)` after a `(when (nil? *project-root*) (throw))`
    // guard reads the default `nil`, not the string it actually holds).
    if value::is_dynamic(s) || is_earmuffed(s) {
        return None;
    }
    let v = super::deps::obs_global(heap, s)?;
    let t = match v {
        Value::Str(id) => Ty::str_lit(&heap.string(id)),
        other => Ty::of_value(other),
    };
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
}

/// Depth cap for [`expr_ty`]. Comfortably below the ~1900-level overflow observed,
/// small enough for the green-process coroutine stack the parallel checker runs
/// on, and far past any real form's nesting. Past it, `expr_ty` returns `None`
/// (unknown → defer) — sound: it only ever *loses* a warning, never invents one.
const MAX_EXPR_TY_DEPTH: u32 = 128;

/// RAII depth counter for [`expr_ty`]: `enter` bumps the thread-local depth and
/// yields `None` at [`MAX_EXPR_TY_DEPTH`] (so `expr_ty` bails); `Drop` restores it.
struct DepthGuard;
impl DepthGuard {
    fn enter() -> Option<DepthGuard> {
        EXPR_TY_DEPTH.with(|d| {
            let n = d.get();
            if n >= MAX_EXPR_TY_DEPTH {
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

/// The static type of an expression form *in `ctx`*, or `None` when it can't
/// be pinned. `None` is "unknown" and is never flagged. Self-evaluating
/// literals get their exact tag; a `quote`d datum gets the datum's tag; a call
/// with a known signature gets its result type; a variable returns whatever
/// `ctx` knows about it (typically `None` for a free / global reference).
pub(super) fn expr_ty(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    // Bail (defer) if the type-walk is pathologically deep — overflow guard.
    let _depth = DepthGuard::enter()?;
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
                ctx.declared_value_ty(s)
                    .or_else(|| ctx.inferred_value_ty(s))
                    .or_else(|| {
                        (!ctx.is_file_global(s))
                            .then(|| global_value_ty(heap, s))
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
        // widened inference still passes). Any unknown element → the whole
        // literal falls back to unrefined `vector` (same all-or-nothing
        // strictness `element_union` already had).
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let elems: Option<Vec<Ty>> = items.iter().map(|&it| expr_ty(heap, it, ctx)).collect();
            Some(match elems {
                Some(e) => Ty::tuple_of(e),
                None => Ty::of(Tag::Vector),
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
            let mut fields = std::collections::BTreeMap::new();
            let mut complete = true;
            for (k, v) in heap.map_entries(id) {
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
            if let Some((base, keys)) = path_of(heap, form) {
                if !keys.is_empty() {
                    if let Some(t) = ctx.path_ty(base, &keys) {
                        return Some(t);
                    }
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
                    if value::symbol_is(s, kw::QUOTE) {
                        return items.get(1).map(|&d| Ty::of_value(d));
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
                        // A **same-file inferred** function sig (Pass 2.8): the return the
                        // checker inferred for a `(defn …)` in this file, which `sig_of`'s
                        // loaded-closure path can't see (the file isn't loaded while checked).
                        // After a declaration (authoritative), before the loaded lookup below.
                        if let Some(sg) = ctx.inferred_fn_sig(s) {
                            return Some(sg.ret);
                        }
                        // An **ability op** with a declared `:-> RET` return type. The op
                        // is a generic `defn` whose body is the dispatch machinery, so its
                        // own inferred type is opaque — the declared return is the only
                        // static handle on what `(area shape)` yields, and flowing it here
                        // lets a call result feed the rest of inference (`(+ (area s) 1.0)`).
                        // Sound: the checker already warns when an impl's body doesn't
                        // conform to this return (`walk::check_impl_returns`), so the
                        // declared type is a contract, not a guess.
                        if let Some(info) = ctx.ability() {
                            if let Some(ret) = info.op_ret_of(s) {
                                return Some(ret.clone());
                            }
                        }
                        // The same handle for a MULTIMETHOD's declared `:-> RET`. A
                        // `defmulti` generic is a `defn` whose body is the dispatch
                        // machinery, so its inferred type is just as opaque as an ability
                        // op's, and the declaration is the only thing that can type
                        // `(compare-to a b)` at a call site. Sound for the same reason:
                        // `check_method_returns` warns when a method body does not conform.
                        if let Some(info) = ctx.multi() {
                            if let Some(ret) = info.ret_of(s) {
                                return Some(ret.clone());
                            }
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
                    if let Some(sigs) = declared_heap_overload(heap, s) {
                        let arg_tys: Vec<Option<Ty>> =
                            items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                        return Some(resolve_overload_ret(&sigs, &arg_tys));
                    }
                    sig_of(heap, s).map(|sig| sig.ret)
                }
                _ => None,
            }
        }
        // A string literal → its singleton `str_lit` (B0 — literal-singleton
        // precision). `Ty::of_value` can't do this (no heap to read the bytes), so
        // it's handled here where the heap is in hand; int/bool/keyword singletons
        // come from `of_value` below.
        Value::Str(id) => Some(Ty::str_lit(&heap.string(id))),
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
        return match items.len() {
            4 => branch_union(heap, &[items[2], items[3]], ctx),
            3 => Some(expr_ty(heap, items[2], ctx)?.union(Ty::of(Tag::Nil))),
            _ => None,
        };
    }
    // `(do … last)` → ty(last). Empty `(do)` is `nil`.
    if value::symbol_is(head, kw::DO) {
        return match items.last() {
            Some(&last) if items.len() > 1 => expr_ty(heap, last, ctx),
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
        let binds = let_bindings(heap, *items.get(1)?)?;
        if binds.len() % 2 != 0 {
            return None;
        }
        let mut scope = ctx.clone();
        let mut i = 0;
        while i < binds.len() {
            let Value::Sym(name) = binds[i] else {
                // A destructuring binding — we can't pin the names; the body may
                // depend on them, so defer the whole form.
                return None;
            };
            let rhs_ty = expr_ty(heap, binds[i + 1], &scope);
            scope = scope.bind(name, rhs_ty);
            i += 2;
        }
        let last = *items.last()?;
        if items.len() < 3 {
            return None;
        }
        return expr_ty(heap, last, &scope);
    }
    // `(cond test1 res1 test2 res2 … :else resN)` — union of the *result*
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
    if value::symbol_is(head, kw::AND) {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Bool));
        }
        return branch_union(heap, &items[1..], ctx);
    }
    if value::symbol_is(head, kw::OR) {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Nil));
        }
        return branch_union(heap, &items[1..], ctx);
    }
    None
}

/// The union of the types of several branch result forms, or `None` if *any* of
/// them is unknown — the graceful-degradation rule that keeps the precise
/// return-check from warning on a form whose value isn't fully pinned.
fn branch_union(heap: &Heap, forms: &[Value], ctx: &Ctx) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    for &f in forms {
        // **Recursion inference.** When inferring a function's own signature, a
        // self-recursive call in a branch-result position contributes ⊥ to the union — by
        // induction it returns exactly the function's return type (the fixpoint), so the
        // *other* (base-case) branches determine it. Skipping it is what lets a tail-recursive
        // `(if base acc (self …))` infer its return from `base` instead of deferring on the
        // unknown self-call. Sound: the recursive branch adds nothing the fixpoint doesn't
        // already contain. If every branch is a self-call, `acc` stays `None` → defer.
        if is_inferring_self_call(heap, f, ctx) {
            continue;
        }
        let t = expr_ty(heap, f, ctx)?;
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
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

/// Parse a `let`-family bindings form into a flat `[name val name val …]` vec —
/// accepts both `(…)` lists and `[…]` vectors (the two shapes the reader emits),
/// mirroring `walk::bindings`. `None` for any other shape.
fn let_bindings(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        Value::Nil | Value::Pair(_) => list_items(heap, form),
        _ => None,
    }
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
    let int = Ty::of(Tag::Int);
    let float = Ty::of(Tag::Float);
    let num = Ty::NUMBER;

    // Always-float unary math: `sqrt`/`sin`/`cos`/`tan` return a float even for a
    // perfect square / whole-number argument (`(sqrt 4)` → `2.0`). Only fires on a
    // known numeric argument (a non-numeric one is a separate arg-type error the
    // curated sig already flags).
    //
    // `sqrt` is keyed `math/sqrt`: ADR-227 moved it into `std/math.blsp`, so the bare
    // spelling no longer exists (unbound without an import) and this rule was dead for the
    // spelling that does. `sin`/`cos`/`tan` stay bare — they are still root natives.
    let is_always_float = value::symbol_is(head, "math/sqrt")
        || value::symbol_is(head, "sin")
        || value::symbol_is(head, "cos")
        || value::symbol_is(head, "tan");
    if is_always_float {
        let arg = *items.get(1)?;
        let t = expr_ty(heap, arg, ctx)?;
        return t.is_subtype(&num).then_some(float);
    }

    let is_contagious = value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "/");
    let is_int_closed = value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "quot")
        || value::symbol_is(head, "rem")
        || value::symbol_is(head, "mod")
        // `math/` since ADR-227 — see the `math/sqrt` note above.
        || value::symbol_is(head, "math/abs");
    if !is_contagious && !is_int_closed {
        return None;
    }
    // Every operand must be a known numeric type; one non-numeric / unknown defers.
    // (Zero operands — e.g. a bare `(+)` — also defers, leaving the curated sig.)
    let args = items.get(1..)?;
    if args.is_empty() {
        return None;
    }
    let mut all_int = true;
    let mut any_float = false;
    for &arg in args {
        let t = expr_ty(heap, arg, ctx)?;
        if !t.is_subtype(&num) {
            return None;
        }
        all_int &= t.is_subtype(&int);
        any_float |= t.is_subtype(&float);
    }
    if is_contagious && any_float {
        return Some(float);
    }
    if is_int_closed && all_int {
        return Some(int);
    }
    None
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
    args.chunks_exact(2)
        .map(|pair| match pair[0] {
            Value::Keyword(name) => Some((name, pair[1])),
            _ => None,
        })
        .collect()
}
/// Element-aware result type for the sequence builtins — `None` falls through to
/// the callee's flat signature. `(list …)`/`(vector …)` build a refined
/// sequence; `(first xs)`/`(last xs)`/`(nth xs i)` extract the element type
/// (widened with `nil` for the empty / out-of-range case, so the result is a
/// sound superset). Only refines when the element type is actually known.
fn seq_aware_call_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    if value::symbol_is(head, "list") {
        return element_union(heap, &items[1..], ctx).map(Ty::list_of);
    }
    if value::symbol_is(head, "vector") {
        return element_union(heap, &items[1..], ctx).map(Ty::vector_of);
    }
    if value::symbol_is(head, "first")
        || value::symbol_is(head, "last")
        || value::symbol_is(head, "nth")
        || value::symbol_is(head, "second")
        || value::symbol_is(head, "third")
    {
        let arg = *items.get(1)?;
        let coll_ty = expr_ty(heap, arg, ctx)?;
        // A statically-known index into a tuple-typed collection resolves to
        // that *exact* position's type (ADR-128), not just the coarse union
        // every other element access falls back to — `first` = 0, `second` =
        // 1, `third` = 2, `last` = the final position, `nth` reads its own
        // literal-int index argument (a non-literal index can't be resolved
        // this precisely, so it falls through to the union case below).
        if let Some(elems) = coll_ty.tuple_elems() {
            let idx = if value::symbol_is(head, "first") {
                Some(0)
            } else if value::symbol_is(head, "second") {
                Some(1)
            } else if value::symbol_is(head, "third") {
                Some(2)
            } else if value::symbol_is(head, "last") {
                elems.len().checked_sub(1)
            } else if value::symbol_is(head, "nth") {
                match items.get(2) {
                    Some(Value::Int(n)) if *n >= 0 => Some(*n as usize),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(i) = idx {
                // In range → exactly that position's type, no `nil` — a
                // tuple's arity is fixed and known, so an in-range access on a
                // well-typed value is never absent. A provably out-of-range
                // literal index → exactly `nil` (matches the runtime, which
                // returns nil rather than erroring).
                return Some(match elems.get(i) {
                    Some(t) => t.clone(),
                    None => Ty::of(Tag::Nil),
                });
            }
        }
        let elem = coll_ty.elem_ty()?;
        // first/second/third/last/nth yield `nil` on an empty / out-of-range seq.
        return Some(elem.union(Ty::of(Tag::Nil)));
    }
    // `(filter pred coll)` keeps `coll`'s element type — the result is the items
    // that pass, so `nil | list<A>` for `A = elem(coll)` (ADR-078 parametric
    // results). `None` element → fall through to the flat curated `list`.
    if value::symbol_is(head, "filter") {
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // Element-preserving reshapers whose sequence is the *first* argument — the
    // same elements, fewer / reordered: `reverse`, `rest` (drop the head),
    // `but-last`, `distinct` / `dedupe` (drop duplicates). `nil | list<A>`.
    if value::symbol_is(head, "reverse")
        || value::symbol_is(head, "rest")
        || value::symbol_is(head, "but-last")
        || value::symbol_is(head, "distinct")
        // `seq/` since ADR-227; `distinct` stayed in the core protocol.
        || value::symbol_is(head, "seq/dedupe")
    {
        let coll = *items.get(1)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // `(sort coll)` / `(sort less? coll)` and `(sort-by key-fn coll)` — the
    // sequence is always the last argument; element type is preserved unchanged.
    if value::symbol_is(head, "sort") || value::symbol_is(head, "sort-by") {
        let coll = *items.last()?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // Element-preserving slices/filters whose sequence is the *second* argument —
    // `take` / `drop` / `take-while` / `drop-while`, `take-last` / `drop-last`, and
    // `remove` (the `filter` complement). Element type is preserved unchanged.
    if value::symbol_is(head, "take")
        || value::symbol_is(head, "drop")
        || value::symbol_is(head, "take-while")
        || value::symbol_is(head, "drop-while")
        || value::symbol_is(head, "take-last")
        || value::symbol_is(head, "drop-last")
        || value::symbol_is(head, "remove")
    {
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // `(cons x xs)` — prepend `x` onto `xs`; the result element type is
    // `type(x) | elem(xs)`. Both must be known; if either is unknown the element
    // type is unknown (the tail may hold values of any type). The result is always
    // a `pair` (not nil), so we return `list<E>` without the `nil` variant.
    if value::symbol_is(head, "cons") && items.len() == 3 {
        let hd_ty = expr_ty(heap, items[1], ctx);
        let tail_elem = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty());
        return match (hd_ty, tail_elem) {
            (Some(h), Some(e)) => Some(Ty::list_of(h.union(e))),
            _ => Some(Ty::of(Tag::Pair)), // one side unknown → unrefined pair
        };
    }
    // `(append xs ys …)` — variadic list concatenation. (`concat` was an alias and
    // was removed; one spelling each.)
    // Result element type is the union of every argument's element type; any
    // argument with an unknown element type → fall through to the flat result.
    if value::symbol_is(head, "append") {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Nil)); // (append) = nil
        }
        let mut acc: Option<Ty> = None;
        for &arg in &items[1..] {
            let elem = expr_ty(heap, arg, ctx).and_then(|t| t.elem_ty())?;
            acc = Some(match acc {
                Some(a) => a.union(elem),
                None => elem,
            });
        }
        return list_result(acc);
    }
    // Map K/V refinement rules — derive result types when the first argument is a
    // `map<K, V>`.  Sound by the usual "widening is conservative" rule: these rules
    // only fire when K/V are known; unknown → fall through to the curated flat result.
    //
    // `(get m k [default])` → `V | nil` (nil = key absent or default not given).
    // On a record shape (Step 5+, ADR-115) with a *literal keyword* key, the
    // exact declared field type wins — more specific than the flat `map_kv`
    // fallback below. An undeclared/dynamic key (or a non-keyword key on a
    // record) falls through: records are open, so an unknown key's type is
    // genuinely unknown, not an error.
    if value::symbol_is(head, "get") && items.len() >= 3 {
        let map_arg = *items.get(1)?;
        let map_ty = expr_ty(heap, map_arg, ctx);
        if let Value::Keyword(key) = items[2] {
            if let Some(fty) = map_ty.as_ref().and_then(|t| t.record_field_ty(key)) {
                return Some(fty);
            }
        }
        if let Some((_, v)) = map_ty.as_ref().and_then(Ty::map_kv) {
            return Some(v.clone().union(Ty::of(Tag::Nil)));
        }
    }
    // `(keys m)` → `nil | list<K>`. On a **closed** record shape the keys are exactly
    // the declared field names, so the element type is that literal set — which is what
    // makes `(keys r)` usable as a value rather than an opaque `list<any>`. An OPEN
    // record may carry keys nothing declares, so it falls through to the `map_kv` rule.
    if value::symbol_is(head, "keys") && items.len() == 2 {
        let map_arg = *items.get(1)?;
        let map_ty = expr_ty(heap, map_arg, ctx);
        if let Some(shape) = closed_record_fields(map_ty.as_ref()) {
            let names = shape
                .keys()
                .fold(Ty::NEVER, |acc, name| acc.union(Ty::keyword_lit(*name)));
            // An optional field may be absent, so the union over ALL declared names is
            // an over-approximation of the keys actually present — sound, and the only
            // answer available without knowing the value.
            return list_result(Some(names));
        }
        if let Some((k, _)) = map_ty.as_ref().and_then(Ty::map_kv) {
            return list_result(Some(k.clone()));
        }
    }
    // `(vals m)` → `nil | list<V>`. On a closed record, every value present is one of
    // the declared field types.
    if value::symbol_is(head, "vals") && items.len() == 2 {
        let map_arg = *items.get(1)?;
        let map_ty = expr_ty(heap, map_arg, ctx);
        if let Some(shape) = closed_record_fields(map_ty.as_ref()) {
            let types = shape
                .values()
                .fold(Ty::NEVER, |acc, (ty, _required)| acc.union(ty.clone()));
            return list_result(Some(types));
        }
        if let Some((_, v)) = map_ty.as_ref().and_then(Ty::map_kv) {
            return list_result(Some(v.clone()));
        }
    }
    // `(dissoc m k …)` → the same record shape without those fields. Exact on a closed
    // record with literal-keyword keys: the result definitely lacks them.
    if value::symbol_is(head, "dissoc") && items.len() >= 3 {
        let map_arg = *items.get(1)?;
        let map_ty = expr_ty(heap, map_arg, ctx);
        if let Some(shape) = closed_record_fields(map_ty.as_ref()) {
            if let Some(removed) = literal_keyword_args(&items[2..]) {
                let mut fields = shape.clone();
                for name in removed {
                    fields.remove(&name);
                }
                return Some(Ty::record_of(fields));
            }
        }
    }
    // `(assoc m k1 v1 …)` → `map<K, V>` with the assoc'd keys and values UNIONED into
    // the refinement. Carrying `K`/`V` forward unchanged — which this did, on the stated
    // grounds of "no false-positive risk either way" — is not sound in the direction
    // that matters: `(assoc m :extra "text")` on a `(map keyword int)` genuinely holds a
    // string at `:extra`, so claiming the result is still `(map keyword int)` made
    // `(get … :extra)` read `nil | int` and flagged correct code. A key or value whose
    // own type is unknown widens that side to `any` rather than keeping a refinement
    // the value may contradict.
    if value::symbol_is(head, "assoc") && items.len() >= 4 && (items.len() - 2).is_multiple_of(2) {
        let map_arg = *items.get(1)?;
        let map_ty = expr_ty(heap, map_arg, ctx);
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
        }
        if let Some((k, v)) = map_ty.as_ref().and_then(Ty::map_kv) {
            let mut key_ty = k.clone();
            let mut val_ty = v.clone();
            for pair in items[2..].chunks_exact(2) {
                key_ty = key_ty.union(expr_ty(heap, pair[0], ctx).unwrap_or(Ty::ANY));
                val_ty = val_ty.union(expr_ty(heap, pair[1], ctx).unwrap_or(Ty::ANY));
            }
            return Some(Ty::map_of(key_ty, val_ty));
        }
    }
    // `(map f coll)` → `nil | list<B>`, `B` = the callback's return type applied
    // to `coll`'s element type. Unknown callback / element → flat `list`.
    if value::symbol_is(head, "map") {
        let f = *items.get(1)?;
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[a], ctx);
        return list_result(b);
    }
    // `(keep f coll)` — `map` then drop the `nil` results; `nil | list<B>` for
    // `B` = the callback's return (over-approximated by keeping `nil` in `B`, a
    // sound superset). Unknown callback / element → flat.
    if value::symbol_is(head, "keep") {
        let f = *items.get(1)?;
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[a], ctx);
        return list_result(b);
    }
    // `(seq/interpose sep coll)` — weave `sep` between `coll`'s elements; the result
    // holds both, `nil | list<A | type(sep)>`. Both must be known, else flat.
    // Keyed qualified: `seq/` since ADR-227.
    if value::symbol_is(head, "seq/interpose") && items.len() == 3 {
        let sep_ty = expr_ty(heap, items[1], ctx);
        let a = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty());
        return match (sep_ty, a) {
            (Some(s), Some(e)) => list_result(Some(s.union(e))),
            _ => None,
        };
    }
    // `(range …)` always produces numbers — `nil | list<number>` (empty for an
    // empty range). A sound superset whatever the bound types (int or float).
    if value::symbol_is(head, "range") {
        return list_result(Some(Ty::NUMBER));
    }
    // `(reduce f init coll)` / `(fold f init coll)` reduce to an accumulator typed
    // `ty(init) | B`, where `B` is the 2-arg callback's return (`(f acc x)`). The
    // accumulator can grow across steps, so it's over-approximated as `any` for
    // the callback inference (sound — a superset); the result joins the
    // empty-input case (`init`) with a step result (`B`). The no-init
    // `(reduce f coll)` form starts the accumulator at `coll`'s first element.
    // Both `init` and `B` must be known, else flat.
    if value::symbol_is(head, "reduce") || value::symbol_is(head, "fold") {
        let f = *items.get(1)?;
        let (init_ty, coll) = match items.len() {
            // (fold f init coll) / (reduce f init coll)
            4 => (expr_ty(heap, items[2], ctx), items[3]),
            // (reduce f coll) — initial accumulator is the first element
            3 if value::symbol_is(head, "reduce") => {
                let coll = items[2];
                let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
                (elem, coll)
            }
            _ => return None,
        };
        let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[Some(Ty::ANY), elem], ctx);
        return match (init_ty, b) {
            (Some(i), Some(b)) => Some(i.union(b)),
            _ => None,
        };
    }
    None
}

/// The result type of a list-producing combinator (`map`/`filter`): `nil |
/// list<elem>` — empty input maps/filters to `nil`. `None` element → `None`, so
/// the caller falls back to the flat curated `list` (never a too-narrow result).
fn list_result(elem: Option<Ty>) -> Option<Ty> {
    elem.map(|e| Ty::list_of(e).union(Ty::of(Tag::Nil)))
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
fn callback_ret(heap: &Heap, f: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    match f {
        // A local binding shadows the global table — its return type isn't known.
        Value::Sym(s) if ctx.is_local(s) => None,
        Value::Sym(s) => {
            // An overloaded callback (ADR-116) — resolve per matching arm from
            // `inputs` instead of a single flat `ret`, same as the call-form case.
            if let Some(sigs) = declared_heap_overload(heap, s) {
                return Some(resolve_overload_ret(&sigs, inputs));
            }
            sig_of(heap, s).map(|sig| sig.ret)
        }
        Value::Pair(_) => lambda_ret(heap, f, inputs, ctx),
        _ => None,
    }
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
    // Exactly `(fn <param-list> <body>)` — one param list + one body expression.
    let parts = &items[1..];
    if parts.len() != 2 {
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
        sub = sub.bind(*p, input.clone());
    }
    expr_ty(heap, parts[1], &sub)
}
