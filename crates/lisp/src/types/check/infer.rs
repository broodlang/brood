//! Type inference over expressions — expr_ty + result-typing helpers
//! (extracted from guards.rs, file-organization split).
use super::ctx::{resolve_overload_ret, Ctx};
use super::guards::path_of;
use super::sigs::{declared_heap_overload, sig_of};
use super::walk::{is_fn_head, list_items};
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::{Sig, Ty};
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
    /// [`expr_ty`]'s memo for the walk in progress: `(form, ctx identity)` → answer.
    /// Owned by the OUTERMOST `expr_ty` call on the thread and cleared when it returns.
    /// Sound within a walk because a `Ctx` never changes under a `&Ctx` and every
    /// binding change is a fresh object with a fresh id (`ctx::CtxId`). What it fixes:
    /// `expr_ty` of a call form types its operands in more than one fallback — a
    /// `seq_aware_call_ty` that answers `None` for an unknown collection, then the
    /// declared-sig path typing the same operand again — which is 2^depth on
    /// `(first (first … x))`: 40 deep hung `nest check`, and only the depth cap ended
    /// it (2026-08-30 audit, C-class). One answer per (form, scope) per walk instead.
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

/// Run `f` with [`expr_ty`]'s depth counter at 0, restoring it after. For a walk that
/// is a FRESH question — a callee's body under `infer_sig`/`specialized_ret` — not an
/// operand of the form that asked it: with the caller's depth inherited, the cap tripped
/// *because of the caller*, `expr_ty` answered `None`, and `infer_sig` memoized that as
/// the callee's signature for the rest of the file (2026-08-30 audit, D2). The native
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
    // Bail (defer) if the type-walk is pathologically deep — overflow guard.
    let _depth = DepthGuard::enter()?;
    let key = match form {
        Value::Pair(id) => Some((id, ctx.id())),
        _ => None,
    };
    if let Some(k) = key {
        if let Some(hit) = EXPR_MEMO.with(|m| m.borrow().get(&k).cloned()) {
            return hit;
        }
    }
    let owner = !EXPR_MEMO_OWNED.with(|o| o.replace(true));
    // The cap bounds the DEPTH; the frame's SIZE is the compiler's, and a debug frame of
    // this function times [`MAX_EXPR_TY_DEPTH`] outgrew the walker's 1 MB stacker segment
    // once call-site specialization landed (2026-08-30: the deep-forms test overflowed
    // at exactly 128 frames). Grow the stack here too, so the cap is the only limit.
    let out = stacker::maybe_grow(64 * 1024, 1024 * 1024, || expr_ty_inner(heap, form, ctx));
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
                    // body typed with its parameters unknown. It used to have no type at
                    // all (`reflect/expr-type` answered nil for every lambda), so it was
                    // `dynamic()` everywhere. Only when `R` is known: an arrow with an
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
                            let elems = list_items(heap, d)?;
                            let mut acc = Ty::NEVER;
                            for &e in &elems {
                                acc = acc.union(quoted_datum_ty(heap, e));
                            }
                            return Some(Ty::list_of(acc));
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
                        // …and the same for a sig a LOADED module declared (the live image:
                        // hover, `reflect/expr-type`, a cross-module call). Skipped for a
                        // name this file redefines — the heap describes the old binding
                        // (ADR-123: a def always wins).
                        if !ctx.is_file_global(s) {
                            if let Some(sv) = super::sigs::declared_heap_sig_with_vars(heap, s) {
                                let arg_tys: Vec<Option<Ty>> =
                                    items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                                return Some(sv.resolve_ret(&arg_tys));
                            }
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
                            if sg.ret.is_any() {
                                // The body says nothing about its result from its own
                                // parameters (a pass-through, an unconstrained accumulator):
                                // re-type it under THIS call's argument types.
                                if let Some(t) =
                                    super::sigs::specialize_call(heap, s, &items[1..], ctx)
                                {
                                    return Some(t);
                                }
                            }
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
                    match sig_of(heap, s).map(|sig| sig.ret) {
                        Some(t) if !t.is_any() => Some(t),
                        // A loaded function whose flat return is `any` (or that inferred
                        // nothing): re-type its body under this call's argument types.
                        flat => super::sigs::specialize_call(heap, s, &items[1..], ctx).or(flat),
                    }
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
            let rhs_ty = expr_ty(heap, binds[i + 1], &scope);
            match binds[i] {
                Value::Sym(name) => scope = scope.bind(name, rhs_ty),
                // A destructuring binding: each positional binder takes the element type
                // (`super::walk::pattern_bindings`), unknown where it can't be pinned.
                pat => {
                    for (sym, ty) in super::walk::pattern_bindings(heap, pat, rhs_ty.as_ref()) {
                        scope = scope.bind(sym, ty);
                    }
                }
            }
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
    // Exactly, not "any operand": an operand that is NOT the last one is the value only
    // when it short-circuits — falsy for `and`, truthy for `or` — so it contributes that
    // half of its type alone. `(or (string/->number s) -1)` is `number`, never `nil`; the
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
    let num = Ty::NUMBER;
    let float = Ty::of(Tag::Float);

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
    // An extremum hands back ONE OF ITS OPERANDS — `(math/max a b)` is `a` or `b` — so its
    // result is the union of the operand types, not the operator's whole domain (`ordered`,
    // which is what the registry-derived sig says and what made `(+ 1 (math/min i n))` read
    // as `number + ordered` under `--strict`). Every operand must type; one unknown defers to
    // the sig. Sound: the union of the operands is exactly the set of values it can return.
    let is_extremum = value::symbol_is(head, "math/max")
        || value::symbol_is(head, "math/min")
        || value::symbol_is(head, "%max")
        || value::symbol_is(head, "%min");
    if is_extremum {
        let args = items.get(1..)?;
        let mut acc: Option<Ty> = None;
        for &arg in args {
            let t = expr_ty(heap, arg, ctx)?;
            acc = Some(match acc {
                Some(u) => u.union(t),
                None => t,
            });
        }
        return acc;
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
    numeric_result(head, &tys)
}

/// The **additive** ring operators — `+ - inc dec`, i.e. the ring minus `*`. What sets
/// them apart for [`numeric_result`] is that shifting a value by whole numbers cannot
/// change whether it has a denominator, which multiplication can (`(* 2 1/2)` is `1`).
fn is_additive(head: Symbol) -> bool {
    value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "inc")
        || value::symbol_is(head, "dec")
}

/// What a numeric operator is, for [`numeric_result`]: `(contagious, int_closed, ring,
/// division)`. `None` for anything that isn't one of these operators.
fn numeric_op_kind(head: Symbol) -> Option<(bool, bool, bool, bool)> {
    let is_division = value::symbol_is(head, "/");
    // `inc`/`dec` are `(+ n 1)` / `(- n 1)` in the prelude, so they close exactly as `+`
    // and `-` do — over ints, over floats (contagion), over ratios.
    let is_ring = value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "inc")
        || value::symbol_is(head, "dec");
    let is_contagious = is_ring || is_division;
    let is_int_closed = is_ring
        || value::symbol_is(head, "quot")
        || value::symbol_is(head, "rem")
        || value::symbol_is(head, "mod")
        // `math/` since ADR-227 — see the `math/sqrt` note above.
        || value::symbol_is(head, "math/abs");
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
        return Some(int);
    }
    // `(+ 1 1/2)` is 3/2 and can be nothing else — see the doc comment above. Placed
    // before the int|ratio fallback, which is the honest answer only once more than one
    // operand can carry a denominator.
    if is_additive(head) && int_shifted && strict_ratios == 1 {
        return Some(ratio);
    }
    // Ratios close over `+ - *` exactly as ints do, and `/` is exact over them too
    // (`(/ 3/2 1/2)` → 3). This used to fall through to the declared signature — widened
    // to `number | map` for `Num` records — which is a true type for the operator and
    // noise as the answer for an all-numeric expression. `quot`/`rem`/`mod` are integer
    // operations and stay int-only.
    if (is_ring || is_division) && all_int_or_ratio {
        return Some(int_or_ratio);
    }
    if is_contagious {
        return Some(num);
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
    args.as_chunks::<2>()
        .0
        .iter()
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
    // `%vector-ref` is what a destructuring `let`/`match` lowers a `[x y w h]` binder to,
    // behind a length check; out of range it throws rather than yielding nil, so it is
    // exactly the element (a tuple position when the index is literal).
    if value::symbol_is(head, "%vector-ref") && items.len() == 3 {
        let coll_ty = expr_ty(heap, items[1], ctx)?;
        if let (Some(elems), Value::Int(n)) = (coll_ty.tuple_elems(), items[2]) {
            if n >= 0 {
                return elems.get(n as usize).cloned();
            }
        }
        return coll_ty.elem_ty();
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
        // `first`/`last` of a provably non-empty list is an element, full stop; every
        // other access (an index that may run off the end, a seq that may be empty)
        // yields `nil` too — except `(nth coll i default)`, whose absent case IS the
        // default, exactly as `get`'s (`(nth parts 1 "0")` is a string, never nil).
        let always_present = provably_non_empty(&coll_ty)
            && (value::symbol_is(head, "first") || value::symbol_is(head, "last"));
        let absent = if value::symbol_is(head, "nth") && items.len() == 4 {
            expr_ty(heap, items[3], ctx)
        } else {
            None
        };
        return Some(if always_present {
            elem
        } else {
            elem.union(absent.unwrap_or(Ty::of(Tag::Nil)))
        });
    }
    // `(filter pred coll)` keeps `coll`'s element type — the result is the items
    // that pass, so `nil | list<A>` for `A = elem(coll)` (ADR-078 parametric
    // results). `None` element → fall through to the flat curated `list`.
    if value::symbol_is(head, "filter") {
        // Data-first (ADR-308): the collection is argument one.
        let coll = *items.get(1)?;
        let coll_ty = expr_ty(heap, coll, ctx);
        let a = coll_ty.as_ref().and_then(|t| t.elem_ty());
        // `filter` builds a LIST whatever it is handed (a vector in, a list out), so with
        // the element type unknown the result is still `list`, not the curated `seqable` —
        // `(filter pred (file/ls d))` declared `(list string)` is not "seqable ⊄ list".
        return list_result(a).or_else(|| coll_ty.map(|_| Ty::LIST));
    }
    // Element-preserving reshapers whose sequence is the *first* argument — the
    // same elements, fewer / reordered: `reverse`, `rest` (drop the head),
    // `but-last`, `distinct` / `dedupe` (drop duplicates). `nil | list<A>`.
    // `reverse`/`distinct`/`dedupe` keep a non-empty input non-empty; `rest`/`but-last`
    // may empty it — the two groups differ in exactly the `nil`.
    if value::symbol_is(head, "reverse")
        || value::symbol_is(head, "distinct")
        // `seq/` since ADR-227; `distinct` stayed in the core protocol.
        || value::symbol_is(head, "seq/dedupe")
    {
        let coll = *items.get(1)?;
        let coll_ty = expr_ty(heap, coll, ctx);
        let a = coll_ty.as_ref().and_then(|t| t.elem_ty());
        return list_result_over(coll_ty.as_ref(), a);
    }
    if value::symbol_is(head, "rest") || value::symbol_is(head, "but-last") {
        let coll = *items.get(1)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // `(range …)` is "a range of integers" (its own docstring): every argument an int
    // means every element is one. Empty ranges are `nil`.
    if value::symbol_is(head, "range") && items.len() >= 2 {
        let int = Ty::of(Tag::Int);
        let all_int = items[1..]
            .iter()
            .all(|&a| expr_ty(heap, a, ctx).is_some_and(|t| t.is_subtype(&int)));
        // A literal bound proves the range non-empty: `(range 5)`, `(range 2 5)`.
        let non_empty = match (items.get(1), items.get(2), items.len()) {
            (Some(Value::Int(n)), None, 2) => *n > 0,
            (Some(Value::Int(a)), Some(Value::Int(b)), 3) => a < b,
            _ => false,
        };
        return if all_int && non_empty {
            Some(Ty::list_of(int))
        } else if all_int {
            list_result(Some(int))
        } else {
            None
        };
    }
    // `(vec coll)` — the same elements, as a vector. Unknown elements → a bare vector.
    if value::symbol_is(head, "vec") && items.len() == 2 {
        let elem = expr_ty(heap, items[1], ctx).and_then(|t| t.elem_ty());
        return Some(match elem {
            Some(e) => Ty::vector_of(e),
            None => Ty::of(Tag::Vector),
        });
    }
    // `(into target coll)` — `target`'s kind, holding `target`'s elements and `coll`'s.
    // Only the kinds whose element story is plain: a vector, list or set target keeps
    // its elements and gains `coll`'s; a map target is that kind, unrefined (its
    // entries come from pairs).
    if value::symbol_is(head, "into") && items.len() == 3 {
        let target = expr_ty(heap, items[1], ctx)?;
        let added = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty());
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
            return Some(match joined {
                Some(e) => Ty::vector_of(e),
                None => Ty::of(Tag::Vector),
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
        return None;
    }
    // `(conj coll x …)` — `coll`'s kind, its elements plus the `x`s. A vector, list or
    // set carries the elements; a map is its kind, unrefined.
    if value::symbol_is(head, "conj") && items.len() >= 3 {
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
            return Some(match coll.elem_ty() {
                Some(e) => Ty::vector_of(e.union(added)),
                None => Ty::of(Tag::Vector),
            });
        }
        if coll.is_subtype(&Ty::of(Tag::Set)) {
            return Some(match coll.elem_ty() {
                Some(e) => Ty::set_of(e.union(added)),
                None => Ty::of(Tag::Set),
            });
        }
        if coll.is_subtype(&Ty::LIST) {
            // onto nil: a list of the items; onto a list: that list plus the items
            let own = if coll.is_subtype(&Ty::of(Tag::Nil)) {
                Some(Ty::NEVER)
            } else {
                coll.elem_ty()
            };
            return Some(match own {
                Some(e) => Ty::list_of(e.union(added)),
                None => Ty::of(Tag::Pair),
            });
        }
        if coll.is_subtype(&Ty::of(Tag::Map)) {
            return Some(Ty::of(Tag::Map));
        }
        if coll.is_subtype(&Ty::of(Tag::Set)) {
            return Some(Ty::of(Tag::Set));
        }
        return None;
    }
    // `(merge m …)` — a map. (Two records merge into a record whose shape is the
    // right-biased union of theirs, but the lattice's record merge is not written, so
    // the honest answer is the kind — still far from the `any` this used to be.)
    if value::symbol_is(head, "merge") && items.len() >= 2 {
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
        return Some(match shapes {
            Some(shapes) => {
                let mut fields = std::collections::BTreeMap::new();
                let mut open = false;
                for (f, o) in shapes {
                    open |= o;
                    for (k, v) in f {
                        fields.insert(k, v);
                    }
                }
                if open {
                    Ty::record_of_open(fields)
                } else {
                    Ty::record_of(fields)
                }
            }
            None => Ty::of(Tag::Map),
        });
    }
    // `(apply f args…)` — whatever `f` returns: a named global's signature (declared,
    // curated or inferred), or a lambda literal's body with its inputs unknown. The
    // spread arguments are not typed (their count is not even known), so this is the
    // flat return, never an input-resolved one.
    if value::symbol_is(head, "apply") && items.len() >= 3 {
        return match items[1] {
            // A numeric operator spread over a sequence of known elements stays in the
            // operator's closure over those elements — `(apply + ints)` is an `int`.
            Value::Sym(f)
                if !ctx.is_local(f) && numeric_op_kind(f).is_some() && items.len() == 3 =>
            {
                let elem = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty());
                match elem {
                    Some(e) => numeric_result(f, &[e]),
                    None => sig_of(heap, f).map(|sig| sig.ret),
                }
            }
            Value::Sym(f) if !ctx.is_local(f) => sig_of(heap, f).map(|sig| sig.ret),
            Value::Pair(_) => {
                let n = list_items(heap, items[1])
                    .and_then(|l| l.get(1).copied())
                    .and_then(|ps| list_items(heap, ps))
                    .map(|ps| ps.len())?;
                lambda_ret(heap, items[1], &vec![None; n], ctx)
            }
            _ => None,
        };
    }
    // `(sort-by coll key-fn)` — data-first (ADR-308), sequence FIRST.
    if value::symbol_is(head, "sort-by") {
        let coll = *items.get(1)?;
        let coll_ty = expr_ty(heap, coll, ctx);
        let a = coll_ty.as_ref().and_then(|t| t.elem_ty());
        return list_result_over(coll_ty.as_ref(), a);
    }
    // `(sort coll)` / `(sort less? coll)` — variadic in its comparator, so the
    // sequence stays LAST in both arms; element type is preserved unchanged.
    if value::symbol_is(head, "sort") {
        let coll = *items.last()?;
        let coll_ty = expr_ty(heap, coll, ctx);
        let a = coll_ty.as_ref().and_then(|t| t.elem_ty());
        return list_result_over(coll_ty.as_ref(), a);
    }
    // Element-preserving slices/filters whose sequence is the *first* argument
    // (data-first, ADR-308) — `take` / `drop` / `take-while` / `drop-while`,
    // `take-last` / `drop-last`, and `remove` (the `filter` complement). Element type
    // is preserved unchanged.
    // `take`/`drop`/`take-while`/`drop-while` are bare (prelude); `take-last`/`drop-last`/
    // `remove` live in `seq` (ADR-227) and are reached QUALIFIED — matching them bare
    // meant the arm never fired for them at all, and their element type was silently
    // lost. `seq/interpose` below was already keyed qualified for the same reason.
    if value::symbol_is(head, "take")
        || value::symbol_is(head, "drop")
        || value::symbol_is(head, "take-while")
        || value::symbol_is(head, "drop-while")
        || value::symbol_is(head, "seq/take-last")
        || value::symbol_is(head, "seq/drop-last")
        || value::symbol_is(head, "seq/remove")
    {
        let coll = *items.get(1)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // `(cons x xs)` — prepend `x` onto `xs`; the result element type is
    // `type(x) | elem(xs)`. Both must be known; if either is unknown the element
    // type is unknown (the tail may hold values of any type). The result is always
    // a `pair` (not nil), so we return `list<E>` without the `nil` variant.
    if value::symbol_is(head, "cons") && items.len() == 3 {
        let hd_ty = expr_ty(heap, items[1], ctx);
        // A `nil` tail (`'()`, `nil`) contributes no elements at all, so the list's
        // elements are exactly the head's — `(cons 1 '())` is `list<1>`, not a bare `pair`.
        let tail_elem = expr_ty(heap, items[2], ctx).and_then(|t| {
            if t.is_subtype(&Ty::of(Tag::Nil)) {
                Some(Ty::NEVER)
            } else {
                t.elem_ty()
            }
        });
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
        return if any_non_empty {
            acc.map(Ty::list_of)
        } else {
            list_result(acc)
        };
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
        // `(get m k default)`: an ABSENT key yields the default, so the absence case reads
        // as the default's type rather than `nil` — `(get dt :hour 0)` is `int`. A default
        // whose type can't be pinned falls back to the two-argument reading (which admits
        // `nil` — a superset, since it can only over-state the absence case).
        let default_ty = match items.get(3) {
            Some(&d) if items.len() == 4 => expr_ty(heap, d, ctx),
            _ => None,
        };
        if let Value::Keyword(key) = items[2] {
            if let Some(d) = default_ty.as_ref() {
                if let Some(fty) = map_ty
                    .as_ref()
                    .and_then(|t| t.record_field_ty_with_default(key, d))
                {
                    return Some(fty);
                }
            }
            if let Some(fty) = map_ty.as_ref().and_then(|t| t.record_field_ty(key)) {
                return Some(fty);
            }
        }
        if let Some((_, v)) = map_ty.as_ref().and_then(Ty::map_kv) {
            return Some(match default_ty {
                Some(d) => v.clone().union(d),
                None => v.clone().union(Ty::of(Tag::Nil)),
            });
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
            for pair in items[2..].as_chunks::<2>().0 {
                key_ty = key_ty.union(expr_ty(heap, pair[0], ctx).unwrap_or(Ty::ANY));
                val_ty = val_ty.union(expr_ty(heap, pair[1], ctx).unwrap_or(Ty::ANY));
            }
            return Some(Ty::map_of(key_ty, val_ty));
        }
    }
    // `(map coll f)` → `nil | list<B>`, `B` = the callback's return type applied
    // to `coll`'s element type. Unknown callback / element → flat `list`.
    if value::symbol_is(head, "map") {
        let (coll, f) = super::sigs::combinator_args(items)?;
        let coll_ty = expr_ty(heap, coll, ctx);
        let a = coll_ty.as_ref().and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[a], ctx);
        return list_result_over(coll_ty.as_ref(), b);
    }
    // `(keep coll f)` — `map` then drop the `nil` results; `nil | list<B>` for
    // `B` = the callback's return with `nil` REMOVED. Dropping nil is exact, not a
    // guess: `keep`'s body is `(if (nil? y) acc (cons y acc))`, so nil is the one
    // thing that cannot reach the result (`false` still can). Keeping it instead —
    // which this did, as a "sound superset" — made every `keep` result `nil | T`
    // and pushed that nil into the next combinator, which is how a `(fold (keep …)
    // 0 (fn (a b) (> b a)))` drew "argument expects ordered, got nil | int".
    // Unknown callback / element → flat.
    // `keep` is `seq/keep` (ADR-227) — keyed qualified, as `seq/interpose` is.
    if value::symbol_is(head, "seq/keep") {
        let (coll, f) = super::sigs::combinator_args(items)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[a], ctx).map(|t| {
            let non_nil = t.clone().difference(Ty::of(Tag::Nil));
            // A callback that only ever yields nil keeps an empty list, i.e. `nil`.
            // Reporting `list<never>` would be true but useless downstream, so keep
            // the unstripped type in that one case.
            if non_nil.is_never() {
                t
            } else {
                non_nil
            }
        });
        return list_result(b);
    }
    // `(seq/interpose coll sep)` — weave `sep` between `coll`'s elements; the result
    // holds both, `nil | list<A | type(sep)>`. Both must be known, else flat.
    // Keyed qualified: `seq/` since ADR-227.
    if value::symbol_is(head, "seq/interpose") && items.len() == 3 {
        let a = expr_ty(heap, items[1], ctx).and_then(|t| t.elem_ty());
        let sep_ty = expr_ty(heap, items[2], ctx);
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
    // `(reduce coll init f)` / `(fold coll init f)` reduce to an accumulator typed
    // `ty(init) | B`, where `B` is the 2-arg callback's return (`(f acc x)`). The
    // accumulator can grow across steps, so it's over-approximated as `any` for
    // the callback inference (sound — a superset); the result joins the
    // empty-input case (`init`) with a step result (`B`). The no-init
    // `(reduce coll f)` form starts the accumulator at `coll`'s first element.
    // Both `init` and `B` must be known, else flat.
    if value::symbol_is(head, "reduce") || value::symbol_is(head, "fold") {
        let (coll_arg, f) = super::sigs::combinator_args(items)?;
        let (init_ty, coll) = match items.len() {
            // (fold coll init f) / (reduce coll init f)
            4 => (expr_ty(heap, items[2], ctx), coll_arg),
            // (reduce coll f) — initial accumulator is the first element
            3 if value::symbol_is(head, "reduce") => {
                let coll = coll_arg;
                let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
                (elem, coll)
            }
            _ => return None,
        };
        let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        // A numeric operator folded over a numeric sequence stays inside the operator's
        // closure: by induction the accumulator is `init` at first and `(op acc x)` after,
        // so with `init` and every element in a closed set the result is in it too —
        // `(reduce + 0 ints)` is an `int`, not the `number | map` the accumulator-as-`any`
        // path below produces from `+`'s declared signature.
        if let (Value::Sym(fs), Some(i), Some(e)) = (f, &init_ty, &elem) {
            if let Some(t) = numeric_result(fs, &[i.clone(), e.clone()]) {
                return Some(t);
            }
        }
        // Seed the accumulator from `init` and take one step to a fixpoint: with `acc₁ =
        // init ∪ f(init, e)`, if `f(acc₁, e) ⊆ acc₁` then by induction every iterate is in
        // `acc₁` — `(fold (fn (h c) (bit/xor (* h 31) …)) 5381 s)` is an int, where seeding
        // `h` as `any` read `(* h 31)` as `number`. Not stable → the `any`-seeded reading
        // (sound: the accumulator is over-approximated).
        // An unknown element is `any` here — still sound, and the accumulator's own closure
        // (`(fn (m s) (math/max m (string/length s)))` over an untyped `lines`) is what
        // matters.
        if let Some(i) = &init_ty {
            let e = elem.clone().unwrap_or(Ty::ANY);
            if let Some(step) = callback_ret(heap, f, &[Some(i.clone()), Some(e.clone())], ctx) {
                let acc = i.clone().union(step);
                if let Some(again) =
                    callback_ret(heap, f, &[Some(acc.clone()), Some(e.clone())], ctx)
                {
                    if again.is_subtype(&acc) {
                        return Some(acc);
                    }
                }
            }
        }
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

/// Is `t` a provably NON-EMPTY sequence — a `list<T>` (the `pair` tag alone; the empty list
/// is `nil`)? The one length fact the lattice states, and the source of every "no `nil`
/// here" tightening below. A vector or set may be empty whatever its element type.
fn provably_non_empty(t: &Ty) -> bool {
    t.is_subtype(&Ty::of(Tag::Pair))
}

/// [`list_result`] for a combinator that keeps its input's LENGTH class — `map`, `sort`,
/// `reverse`, `distinct`: a provably non-empty input gives a provably non-empty output, so
/// the `nil` case is dropped. `(map inc '(1 2))` is `list<int>`, not `nil | list<int>`. An
/// input that may be empty keeps the `nil`.
fn list_result_over(input: Option<&Ty>, elem: Option<Ty>) -> Option<Ty> {
    if input.is_some_and(provably_non_empty) {
        elem.map(Ty::list_of)
    } else {
        list_result(elem)
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
        // A LEXICAL local (a `let`/`fn` binding) shadows the global table — its return type
        // isn't known. A file global is a local too in `is_local`'s sense, but it is exactly
        // the callee the same-file tables below describe, so the guard is the lexical one.
        Value::Sym(s) if ctx.is_lexical_local(s) => None,
        Value::Sym(s) => {
            // A numeric operator as the callback — `(map inc xs)`, `(reduce + 0 xs)` — with
            // every input known: the same closure rules a direct call gets, instead of the
            // operator's declared signature (widened to `number | map` for `Num` records).
            if let Some(tys) = inputs.iter().cloned().collect::<Option<Vec<Ty>>>() {
                if let Some(t) = numeric_result(s, &tys) {
                    return Some(t);
                }
            }
            // A flat `any` answer is where the callback's own body says nothing about its
            // result FROM ITS OWN PARAMS — `(defn step (acc v) (conj acc …))` reads `any`
            // because `acc` is unconstrained. The combinator knows what it hands over, so
            // re-type the body under `inputs` (call-site specialization) before settling.
            let specialize = |flat: Option<Ty>| -> Option<Ty> {
                match flat {
                    Some(t) if !t.is_any() => Some(t),
                    flat => super::sigs::specialized_ret(heap, s, inputs, ctx).or(flat),
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
                    return specialize(Some(resolve_overload_ret(&sigs, inputs)));
                }
                if let Some(sg) = ctx.inferred_fn_sig(s) {
                    return specialize(Some(sg.ret));
                }
            }
            if ctx.is_file_global(s) {
                return specialize(None);
            }
            // An overloaded callback (ADR-116) — resolve per matching arm from
            // `inputs` instead of a single flat `ret`, same as the call-form case.
            if let Some(sigs) = declared_heap_overload(heap, s) {
                return Some(resolve_overload_ret(&sigs, inputs));
            }
            specialize(sig_of(heap, s).map(|sig| sig.ret))
        }
        Value::Pair(_) => lambda_ret(heap, f, inputs, ctx),
        _ => None,
    }
}

/// The type of one quoted datum: a nested list recurses (`'((1) (2))` is `list<list<1|2>>`),
/// an empty list is `nil`, anything else is its `of_value` singleton/tag.
fn quoted_datum_ty(heap: &Heap, d: Value) -> Ty {
    // Deep-form stack safety: recurses into itself, never back through `expr_ty`'s
    // cap, so a deep quoted datum (a macro-emitted template) would overflow otherwise.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || quoted_datum_ty_inner(heap, d))
}

fn quoted_datum_ty_inner(heap: &Heap, d: Value) -> Ty {
    match d {
        Value::Pair(_) => match list_items(heap, d) {
            Some(elems) => {
                let mut acc = Ty::NEVER;
                for &e in &elems {
                    acc = acc.union(quoted_datum_ty(heap, e));
                }
                Ty::list_of(acc)
            }
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
    // is just an earlier form here.) `try`'s expansion is the case that needed this:
    // `(%try (fn () a b) …)` used to be untypeable because the thunk had two forms.
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
        sub = sub.bind(*p, input.clone());
    }
    expr_ty(heap, *parts.last()?, &sub)
}
