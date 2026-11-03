use super::args::{arg, expect_int, expect_map, expect_rope_ref, expect_set, expect_symbol, two};
use super::realize_seqview;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval::apply;
use std::sync::LazyLock;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // pair / sequence — `empty?` is Brood (type dispatch over string/length /
    // vector-length / map-keys; std/prelude.blsp). `first`/`rest` ARE the pair
    // accessors (car/cdr), so they stay. `rest` always yields a list (a vector's
    // tail is built via `heap.list`), never a vector.
    primitives.def(
        "cons",
        Arity::exact(2),
        Sig::new(vec![any, any], pair),
        &["x", "xs"],
        "A new pair with head x and tail xs.\n\n    (cons 1 (list 2))   ;=> (1 2)",
        cons,
    );
    primitives.def(
        "first",
        Arity::exact(1),
        Sig::new(vec![seqable], any),
        &["coll"],
        "The head of any sequence — a list, vector, bytes, set (an element) or map (a [k v] pair) — or nil if empty.\n\n    (first [1 2 3])   ;=> 1\n    (first {:a 1})    ;=> [:a 1]\n    (first [])        ;=> nil",
        first);
    primitives.def(
        "rest",
        Arity::exact(1),
        Sig::new(vec![seqable], list_ty),
        &["coll"],
        "All but the head of any sequence, as a list (a set yields its remaining elements, a map its remaining [k v] pairs). A one-element sequence yields nil, not an empty list.\n\n    (rest [1 2 3])   ;=> (2 3)\n    (rest [1])       ;=> nil",
        rest);
    primitives.def(
        "nil?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "True if x is nil.\n\n    (nil? nil)   ;=> true",
        is_nil,
    );
    primitives.def(
        "pair?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "True if x is a cons pair.\n\n    (pair? (list 1))   ;=> true",
        is_pair,
    );
    primitives.def(
        "empty?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["coll"],
        "True if coll is empty (nil, an empty string/vector/map, or a seq-view that realises to nothing).\n\n    (empty? [])   ;=> true",
        is_empty);
    // Lazy reducible range (ADR: reducible range). `%range` constructs it (arg
    // parsing is in the Brood `range`); the fold-family fast paths in the prelude
    // call `range?` / `%range-fold-span` / `%range-count`; everything else realises
    // via `%range->list`. A range carries `tag = Pair`, so its surface type is a
    // list — hence the `list_ty` sigs.
    primitives.def(
        "%range",
        Arity::exact(3),
        Sig::new(vec![int, int, int], list_ty),
        &[],
        "",
        range_make,
    );
    primitives.def(
        "range?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "True if x is a lazy range (as produced by range). Ranges fold/reduce/sum/count without materialising; other ops treat them as the list they stand for.\n\n    (range? (range 3))   ;=> true",
        range_pred);
    primitives.def(
        "%range-count",
        Arity::exact(1),
        Sig::new(vec![list_ty], int),
        &[],
        "",
        range_count,
    );
    primitives.def(
        "%range-bounds",
        Arity::exact(1),
        Sig::new(vec![list_ty], vec_ty),
        &["r"],
        "The [lo hi step] of a lazy range. Internal: the compiler's counted-loop rewrite of a fold over a range reads its bounds once through this (ADR-360 §7).",
        range_bounds,
    );
    primitives.def(
        "%range->list",
        Arity::exact(1),
        Sig::new(vec![list_ty], list_ty),
        &[],
        "",
        range_to_list,
    );
    primitives.def(
        "%range-fold-span",
        Arity::exact(5),
        Sig::new(vec![callable, any, list_ty, any, any], any),
        &[],
        "",
        range_fold_span,
    );
    // The vector counterpart of `%range-fold-span`, behind the prelude `fold`'s vector
    // branch: fold the container in a native loop instead of paying a per-element `apply`,
    // with a passthrough reducer like `+` resolved once rather than per element.
    primitives.def(
        "%vector-fold-span",
        Arity::exact(5),
        Sig::new(vec![callable, any, any, any, any], any),
        &[],
        "",
        vector_fold_span,
    );
    // Lazy seq-view (ADR: lazy seq-view) — the fused result of `map`/`filter`/
    // `keep`/`remove`. `%seqview` constructs it from `[source xform]`;
    // `%seqview-parts` returns that pair as a 2-vector for the prelude `fold`
    // fusion / realisation; `seqview?` is the fold-family fast-path predicate.
    // A view carries `tag = Pair` (it is the list it stands in for), hence `pair`.
    primitives.def(
        "%seqview",
        Arity::exact(2),
        Sig::new(vec![any, callable], pair),
        &[],
        "",
        seqview_make,
    );
    primitives.def(
        "%seqview-parts",
        Arity::exact(1),
        Sig::new(vec![any], vec_ty),
        &[],
        "",
        seqview_parts,
    );
    primitives.def(
        "seqview?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "True if x is a lazy sequence view — the reducible produced by range/map/filter/… before it is realized (into/count/…).\n\n    (seqview? [1 2])   ;=> false",
        seqview_pred);
    // `%sort-asc` is the Rust fast path for the common `(sort coll)` case
    // (ascending by `<`, no custom comparator). Avoids per-comparison Brood
    // eval overhead — the old in-Brood mergesort was ~1.5 s on 10 000 items
    // because every compare went through `eval::apply`. `sort-by` /
    // `(sort coll cmp)` still routes through the Brood merge sort for
    // arbitrary comparators. Items must be all-`int` or all-`float`; mixed
    // numerics work by promotion (matches `<`'s semantics).
    primitives.def(
        "%sort-asc",
        Arity::exact(1),
        Sig::new(vec![seq_items_ty], list_ty),
        &[],
        "",
        sort_asc,
    );
    // `%sort-cmp` is the non-numeric fallback for `(sort coll)`: sorts via the
    // Rust-side structural total order (`value_cmp`). Lets `(sort [[1 0] [2 1]])`
    // and the like work without a custom comparator. Brood `sort` (prelude)
    // dispatches: numeric items go through `%sort-asc` (faster), anything else
    // through `%sort-cmp`.
    primitives.def(
        "%sort-cmp",
        Arity::exact(1),
        Sig::new(vec![seq_items_ty], list_ty),
        &[],
        "",
        sort_cmp,
    );
    // `(compare a b)` exposes the same structural total order as a binary
    // comparison (-1/0/1), so `sort-by` / `min-by` / custom comparators work over
    // any orderable value (strings, keywords, vectors, …), not just numbers.
    primitives.def(
        "compare",
        Arity::exact(2),
        Sig::new(vec![any, any], int),
        &["a", "b"],
        "Structural total-order comparison: -1 if a sorts before b, 0 if equal, 1 if after. Numbers numerically; strings/keywords/symbols by text; vectors/lists lexicographically; cross-kind by a stable tag rank. The binary form of `sort`'s order — `sort-by` and custom comparators build on it.\n\n    (compare 1 2)   ;=> -1",
        compare);
    // `(%hash v)` — the structural hash maps and sets key by, as an int. Equal values hash
    // equal, and a symbol or keyword hashes by its spelling, so the number does not depend
    // on interning order. `std/sorted` takes its treap priorities from it, which makes a
    // sorted collection's shape a function of its keys alone (so `=` compares contents).
    primitives.def(
        "%hash",
        Arity::exact(1),
        Sig::new(vec![any], int),
        &["v"],
        "The structural hash of `v` as an integer: equal values hash equal, and a symbol or keyword hashes by its spelling.\n\n    (= (%hash [1 :a]) (%hash [1 :a]))   ;=> true",
        hash);
    // vector
    primitives.def(
        "vector",
        Arity::any(),
        Sig::variadic(any, vec_ty),
        &["&", "items"],
        "A vector of the given items.\n\n    (vector 1 2 3)   ;=> [1 2 3]",
        vector,
    );
    primitives.def(
        "%vector-ref",
        Arity::exact(2),
        Sig::new(vec![vec_ty, int], any),
        &["v", "i"],
        "The element at index i of vector v.",
        vector_ref,
    );
    primitives.def(
        "%vector-length",
        Arity::exact(1),
        Sig::new(vec![vec_ty], int),
        &["v"],
        "The number of elements in vector v.",
        vector_length,
    );
    primitives.def(
        "%vector-assoc",
        Arity::exact(3),
        Sig::new(vec![vec_ty, int, any], vec_ty),
        &["v", "i", "x"],
        "A fresh vector like v with index i (in [0, len)) set to x.",
        vector_assoc,
    );
    primitives.def(
        "%vector-concat",
        Arity::exact(2),
        Sig::new(vec![vec_ty, seqable], vec_ty),
        &["v", "xs"],
        "A fresh vector of v's elements followed by those of xs (a vector, list, range, set or nil), in one allocation. The kernel op behind conj on a vector and into onto one.",
        vector_concat,
    );
    primitives.def(
        "%subvec",
        Arity::range(2, 3),
        Sig::with_rest(vec![vec_ty, int], int, vec_ty),
        &["v", "start", "end"],
        "A fresh vector of v's elements in [start, end); end defaults to the length.",
        subvec,
    );
    // map — the *minimal* kernel: construct, read, two producers, and one
    // enumerator (`%map-pairs` → [k v] vectors). `keys`/`vals`/`contains?`/
    // `map/reduce-kv` and the `get`/`assoc`/`dissoc` surface (variadic + defaults) are
    // all Brood over these (std/prelude/map.blsp, std/map.blsp). Maps are immutable:
    // each op returns a fresh map.
    primitives.def(
        "%hash-map",
        Arity::any(),
        Sig::variadic(any, map_ty),
        &["&", "kvs"],
        "A map from alternating key/value arguments (last wins on duplicate keys).",
        hash_map,
    );
    primitives.def(
        "%map-get",
        Arity::range(2, 3),
        Sig::with_rest(vec![map_ty, any], any, any),
        &["m", "k", "default"],
        "The value at key k in map m, or default (else nil).",
        map_get,
    );
    primitives.def(
        "%map-assoc",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, any], map_ty),
        &["m", "k", "v"],
        "A fresh map like m with key k set to v.",
        map_assoc,
    );
    primitives.def(
        "%map-int-add",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, int], map_ty),
        &["m", "k", "delta"],
        "A fresh map like m with key k's value incremented by the integer delta (inserts delta when k is absent): exactly (assoc m k (+ (get m k 0) delta)) — a float under the key adds as + does and an i64 overflow promotes to a bignum — in a single trie traversal when the stored value is a plain integer. The compiler recognises the idiomatic spelling and treats both the same; prefer writing the assoc/get form.",
        map_int_add);
    primitives.def(
        "%map-dissoc",
        Arity::exact(2),
        Sig::new(vec![map_ty, any], map_ty),
        &["m", "k"],
        "A fresh map like m with key k removed.",
        map_dissoc,
    );
    primitives.def(
        "%map-pairs",
        Arity::exact(1),
        Sig::new(vec![map_ty], list_ty),
        &["m"],
        "The entries of m as a list of [k v] vectors, in insertion order.",
        map_pairs,
    );
    primitives.def(
        "%map-count",
        Arity::exact(1),
        Sig::new(vec![map_ty], int),
        &["m"],
        "The number of entries in map m. O(1) — the CHAMP root tracks its size.",
        map_count,
    );
    primitives.def(
        "%map-into",
        Arity::exact(2),
        Sig::new(vec![map_ty, any], map_ty),
        &[],
        "",
        map_into,
    );
    // Ability dispatch through the per-op inline cache (ADR-172 §7): (impls, op-key, id) →
    // impl fn or nil. Internal; the op `defability` emits calls it, never user code.
    primitives.def(
        "%dispatch",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, any], any),
        &[],
        "",
        dispatch,
    );
    // The head of a devirtualized ability call (ADR-182/294): `%dispatch` behind a guard
    // that the op (and a constructor receiver's constructor) is still what the compiler
    // proved. Internal; only `inline::mono_devirtualize` emits it.
    primitives.def(
        "%dispatch-proven",
        Arity::range(5, 7),
        Sig::new(vec![any, any, map_ty, any, any, any, any], any),
        &[],
        "",
        dispatch_proven,
    );
    // Atomic registry update (KI-22): the read-modify-write of a global holding a whole
    // registry map, done in ONE kernel call so two concurrent registrations cannot each
    // read the old map and clobber each other. Internal; `register-impl`/`provide`/
    // `defability`/… in the prelude call it, never user code.
    // The optional fifth argument names the module making the write for the KI-136 writer
    // journal (`%registry-writer`); an image replay passes the module it is replaying.
    primitives.def(
        "%registry-update!",
        Arity::range(4, 5),
        Sig::new(vec![any, any, any, any, any], any),
        &[],
        "",
        registry_update,
    );
    // Who wrote a registry entry (KI-136): the module whose load made it, or nil for an
    // entry nobody owns (a prelude/root write). What lets the stdlib image carry each
    // registration WITH the module that made it rather than with the registry's own.
    primitives.def(
        "%registry-writer",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        &[],
        "",
        registry_writer,
    );
    // The general form of the above (KI-23): compare-and-swap, for a registry whose update
    // is not a single map/list op. Lets the transform stay a Brood function while the
    // read-decide-write stays indivisible; `registry-swap!` in the prelude retries on it.
    primitives.def(
        "%registry-cas!",
        Arity::exact(3),
        Sig::new(vec![any, any, any], any),
        &[],
        "",
        registry_cas,
    );
    // Cache-bypassing membership test for a registry map (ADR-225): `require`'s load-once
    // guard must never miss a racing loader's `provide` (which the per-process inline cache
    // can momentarily hide) and reload the module. Reads the shared globals table directly.
    primitives.def(
        "%registry-member?",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        &[],
        "",
        registry_member,
    );
    // Which globals the two above have actually written (ADR-218): the derived answer to
    // "what does loading MUTATE rather than create?", which the startup image needs and the
    // `(reflect/global-names)` diff cannot see. Naming them by hand went stale twice, silently.
    primitives.def(
        "%registry-names",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "Every global a registry update (%registry-update! / %registry-cas!) has written in this runtime, sorted by spelling. The derived answer to \"which globals does LOADING mutate rather than create?\" — the ones a startup image has to carry deliberately, because the (reflect/global-names) diff it is built from cannot see them (ADR-218). Naming them by hand went stale three times, silently; std/tool/project.blsp filters this instead.",
        registry_names);
    // set (the `#{…}` kernel type; the `set` library is Brood over these)
    primitives.def(
        "%set",
        Arity::at_least(0),
        Sig::variadic(any, set_ty),
        &["&", "xs"],
        "Build a set from the element args (the programmatic form of the `#{ }` literal). Dedups by structural equality. The `set` library's constructor is Brood over this.",
        set_construct);
    primitives.def(
        "%set-add",
        Arity::exact(2),
        Sig::new(vec![set_ty, any], set_ty),
        &["s", "x"],
        "A fresh set like s with element x added (a set already holding x is returned unchanged). O(log n).",
        set_add);
    primitives.def(
        "%set-remove",
        Arity::exact(2),
        Sig::new(vec![set_ty, any], set_ty),
        &["s", "x"],
        "A fresh set like s with element x removed (absent → unchanged). O(log n).",
        set_remove,
    );
    primitives.def(
        "%set-has?",
        Arity::exact(2),
        Sig::new(vec![set_ty, any], bool_ty),
        &["s", "x"],
        "Is x an element of set s? O(log n).",
        set_has,
    );
    primitives.def(
        "%set-count",
        Arity::exact(1),
        Sig::new(vec![set_ty], int),
        &["s"],
        "The number of elements in set s. O(1) — the CHAMP root tracks its size.",
        set_count,
    );
    // string
    primitives.def(
        "string/length",
        Arity::exact(1),
        Sig::new(vec![string], int),
        &["s"],
        "The number of characters in string s.\n\n    (string/length \"Hi there\")   ;=> 8",
        string_length,
    );
    primitives.def(
        "string/display-width",
        Arity::range(1, 3),
        Sig::with_optional(vec![string], vec![int, int], int),
        &["s", "start-col", "tab-width"],
        "How many terminal/grid cells string s occupies (grapheme-cluster aware: an emoji / flag / CJK char counts as 2, a combining mark 0, a tab the cells to its next stop). The width-aware counterpart to string/length. The optional start-col (default 0) is the column s is laid out from — a chunk of a longer line passes its column so its tabs land on the LINE's stops — and tab-width the distance between stops — by default the dynamic *tab-width* (8 unless a binding says otherwise).\n\n    (string/display-width \"Hi there\")   ;=> 8\n    (binding (*tab-width* 4) (string/display-width \"\\tx\"))   ;=> 5\n    (string/display-width \"a\\tb\")       ;=> 9\n    (string/display-width \"\\tx\" 3)      ;=> 6",
        display_width);
    // type reflection — the tag predicates (nil?/int?/string?/…) are Brood
    // (std/prelude.blsp) over this one reflective primitive.
    primitives.def(
        "type-of",
        Arity::exact(1),
        Sig::new(vec![any], kw),
        &["x"],
        "The runtime type of x as a keyword (:int, :string, :pair, ...).\n\n    (type-of :k)   ;=> :keyword",
        type_of,
    );
}

// ---------- pair / sequence ----------

pub(super) fn cons(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(heap.alloc_pair(a, b))
}

/// Realise any lazy seq-view among `args`, returning the arguments with views
/// replaced by their realised lists (non-view args untouched). For the
/// stringifiers/printers, whose `&Heap` printer can't run a transducer. Fast path:
/// no view ⇒ `args` itself, borrowed — no copy, no eval. Rooting: each `realize_seqview` can collect,
/// so every input and every already-realised result is kept on the root stack.
pub(super) fn realize_seqviews<'a>(
    heap: &mut Heap,
    env: EnvId,
    args: &'a [Value],
) -> Result<std::borrow::Cow<'a, [Value]>, LispError> {
    if !args.iter().any(|a| matches!(a, Value::SeqView(_))) {
        return Ok(std::borrow::Cow::Borrowed(args));
    }
    heap.root_scope(|heap| {
        let in_roots: Vec<_> = args.iter().map(|&a| heap.root(a)).collect();
        let mut out_roots: Vec<_> = Vec::with_capacity(args.len());
        for r in &in_roots {
            let v = heap.read_root(*r);
            let v = if matches!(v, Value::SeqView(_)) {
                realize_seqview(heap, env, v)?
            } else {
                v
            };
            out_roots.push(heap.root(v));
        }
        Ok(std::borrow::Cow::Owned(
            out_roots.iter().map(|r| heap.read_root(*r)).collect(),
        ))
    })
}

/// If `v` is a RECORD (a `Value::Map` carrying `:__id__`), return its `Seqable` view — the
/// list its `->seq` ability op yields (its fields id-free by default, or a custom
/// collection's own sequence). Lets `first`/`rest`/`empty?` treat a record AS its sequence
/// (ADR-172 §7). Only reached on the builtin fallback: `first`/`rest` are `PrimOp1`s the JIT
/// inlines for lists, so the hot `fold--loop` never calls these. Returns `None` for a
/// non-record, so the caller keeps its normal path (a plain map stays a map).
pub(super) fn record_seq(heap: &mut Heap, v: Value) -> Result<Option<Value>, LispError> {
    let m = match v {
        Value::Map(m) => m,
        _ => return Ok(None),
    };
    if heap
        .map_get(m, Value::Keyword(*crate::core::value::RECORD_ID_SYMBOL))
        .is_none()
    {
        return Ok(None);
    }
    let genv = heap.global();
    let callee = heap
        .env_get(genv, crate::core::value::intern("->seq"))
        .ok_or_else(|| LispError::runtime("->seq: the Seqable protocol is unavailable"))?;
    Ok(Some(crate::eval::compile::apply_engine(
        heap,
        callee,
        &[v],
        genv,
    )?))
}

/// `first` / `rest` / `empty?` of a RECORD, handed back to the engine that offered to make
/// the call (ADR-505): `shim` is the prelude's `%record-first` / `%record-rest` /
/// `%record-empty?`, which asks the record's `->seq` (or `->count`) and answers as the builtin
/// does — called by the engine, so a `receive` in the record's method parks. `None` when `v`
/// is no record, the call was not offered, or the prelude has not defined the shim yet; the
/// builtin then asks the record here, nested ([`record_seq`]).
fn record_redirect(heap: &mut Heap, args: &[Value], shim: &str, v: Value) -> Option<LispError> {
    let Value::Map(m) = v else {
        return None;
    };
    heap.map_get(m, Value::Keyword(*crate::core::value::RECORD_ID_SYMBOL))?;
    let genv = heap.global();
    let callee = heap.env_get(genv, crate::core::value::intern(shim))?;
    if !heap.redirect_offered(args) {
        return None;
    }
    Some(heap.redirect(callee, &[v]))
}

/// `first` / `rest` for the JIT's non-pair fallback (`brood_rt_first`/`_rest`): the cases
/// the kernel decides on its own — nil, vector, range, bytes, set, and the type error —
/// answered; `None` for the two that need the evaluator (a record's `Seqable` dispatch,
/// a lazy seq-view's realisation), which the caller turns into a deopt so the VM's own
/// `first`/`rest` runs them. Everything else here is exactly the builtin.
#[cfg(feature = "jit")]
pub(crate) fn first_without_eval(heap: &mut Heap, v: Value) -> Option<LispResult> {
    if matches!(v, Value::Map(_) | Value::SeqView(_)) {
        return None;
    }
    let genv = heap.global();
    Some(first(&[v], genv, heap))
}

/// See [`first_without_eval`].
#[cfg(feature = "jit")]
pub(crate) fn rest_without_eval(heap: &mut Heap, v: Value) -> Option<LispResult> {
    if matches!(v, Value::Map(_) | Value::SeqView(_)) {
        return None;
    }
    let genv = heap.global();
    Some(rest(&[v], genv, heap))
}

/// `empty?` for the JIT's non-nil/non-pair fallback (`brood_rt_is_empty`) — see
/// [`first_without_eval`], whose split this mirrors: a vector, string, set, bytes, range,
/// rope or table answers from its own length; a map (a record's `Seqable` view decides,
/// and a record IS a map) and a lazy seq-view (it realises) need the evaluator and deopt.
#[cfg(feature = "jit")]
pub(crate) fn is_empty_without_eval(heap: &mut Heap, v: Value) -> Option<LispResult> {
    if matches!(v, Value::Map(_) | Value::SeqView(_)) {
        return None;
    }
    let genv = heap.global();
    Some(is_empty(&[v], genv, heap))
}

pub(crate) fn first(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v0 = arg(args, 0);
    if let Some(redirect) = record_redirect(heap, args, "%record-first", v0) {
        return Err(redirect);
    }
    // a record dispatches to its `Seqable` view first (custom collection or fields).
    let v = match record_seq(heap, v0)? {
        Some(s) => s,
        None => v0,
    };
    match v {
        Value::Pair(p) => Ok(heap.car(p)),
        Value::Vector(id) => Ok(heap.vector(id).first().copied().unwrap_or(Value::nil())),
        // Bytes are a sequence of ints 0–255; the head byte, or nil if empty.
        Value::Bytes(id) => Ok(heap
            .bytes(id)
            .as_bytes()
            .first()
            .map(|&b| Value::int(b as i64))
            .unwrap_or(Value::nil())),
        // A range is non-empty by construction, so its head is `lo`.
        Value::Range(id) => Ok(Value::int(heap.range_parts(id).0)),
        // A lazy seq-view realises (running its transducer) then yields the head
        // of the resulting list. Rare — the prelude routes most consumers through
        // `seq`/`fold`; this serves a direct `(first (map f xs))`.
        Value::SeqView(_) => match realize_seqview(heap, env, v)? {
            Value::Pair(p) => Ok(heap.car(p)),
            _ => Ok(Value::nil()),
        },
        // A set is a sequence of its elements (CHAMP order): its head, or nil if
        // empty — so `first`/`map`/`fold`/… treat a set as a seq (Clojure-like).
        // The set shares the map's CHAMP store (keys are the elements), so its head is the
        // first entry's key — the same element `set_elems` lists first, without copying
        // every element out to take one.
        Value::Set(id) => Ok(heap
            .map_first_entry(id)
            .map(|(element, _)| element)
            .unwrap_or(Value::nil())),
        // A map seqs as its `[k v]` pairs — the same view `seq`/`map`/`fold`/`last`
        // already take, so `first`/`rest` accept what every other seq op does.
        Value::Map(id) => match heap.map_first_entry(id) {
            Some((k, val)) => Ok(heap.alloc_vector2(k, val)),
            None => Ok(Value::nil()),
        },
        Value::Nil => Ok(Value::nil()),
        _ => Err(LispError::wrong_type(
            heap,
            "first",
            "list, vector, set, map or bytes",
            v,
        )),
    }
}

pub(crate) fn rest(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v0 = arg(args, 0);
    if let Some(redirect) = record_redirect(heap, args, "%record-rest", v0) {
        return Err(redirect);
    }
    let v = match record_seq(heap, v0)? {
        Some(s) => s,
        None => v0,
    };
    match v {
        Value::Pair(p) => Ok(heap.cdr(p)),
        Value::Vector(id) => {
            let items: Vec<Value> = heap.vector(id).iter().skip(1).copied().collect();
            Ok(heap.list(items))
        }
        // The tail of a range is another range, one step in — no materialisation
        // (`alloc_range` returns `Nil` once it's empty).
        Value::Range(id) => {
            let (lo, hi, step) = heap.range_parts(id);
            // The next start can leave i64 near MIN/MAX — `(rest (%range 1 2 i64::MAX))`
            // — and the range is exhausted at exactly that point, so yield the empty
            // range instead of a wrapped `lo`. Matches `range_to_vec`/`range_eq_list`,
            // which end their walk on the same `checked_add` miss. (An unchecked `+`
            // here panicked under debug-assertions and silently produced a garbage
            // range in release.)
            match lo.checked_add(step) {
                Some(next) => Ok(heap.alloc_range(next, hi, step)),
                None => Ok(Value::nil()),
            }
        }
        // The tail of a bytes value is a fresh bytes value (all but the first byte).
        Value::Bytes(id) => {
            let tail: Vec<u8> = heap.bytes(id).as_bytes().iter().skip(1).copied().collect();
            Ok(heap.alloc_bytes(crate::core::blob::SharedBlob::new(&tail)))
        }
        // A lazy seq-view realises then yields the tail of the resulting list.
        Value::SeqView(_) => match realize_seqview(heap, env, v)? {
            Value::Pair(p) => Ok(heap.cdr(p)),
            _ => Ok(Value::nil()),
        },
        // The tail of a set is a plain list of its remaining elements (CHAMP order):
        // a set seqs as its elements, and after the first `rest` the fold walks a
        // list — so a `(fold f init a-set)` materialises the set at most once (O(n)).
        Value::Set(id) => {
            let items: Vec<Value> = heap.set_elems(id).into_iter().skip(1).collect();
            Ok(heap.list(items))
        }
        // The tail of a map is a plain list of its remaining `[k v]` pairs — the
        // set arm's reasoning, over the map's entry view (see `first`).
        Value::Map(id) => {
            let entries = heap.map_entries(id);
            Ok(entry_pair_list(heap, entries.get(1..).unwrap_or(&[])))
        }
        Value::Nil => Ok(Value::nil()),
        _ => Err(LispError::wrong_type(
            heap,
            "rest",
            "list, vector, set, map or bytes",
            v,
        )),
    }
}

pub(super) fn is_nil(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(arg(args, 0), Value::Nil)))
}

pub(super) fn is_pair(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(
        arg(args, 0),
        Value::Pair(_) | Value::Range(_) | Value::SeqView(_)
    )))
}

/// A RECORD's own count through its `Counted` impl, or `None` when `v` is not a record or its
/// type does not implement `Counted` (then the caller walks the `Seqable` view, as before).
/// Lets `empty?` answer a queue in O(1) instead of building its sequence. Cold path only, as
/// [`record_seq`] is.
fn record_counted(heap: &mut Heap, v: Value) -> Result<Option<Value>, LispError> {
    use crate::core::value::intern;
    let Value::Map(m) = v else {
        return Ok(None);
    };
    let Some(id) = heap.map_get(m, Value::Keyword(*crate::core::value::RECORD_ID_SYMBOL)) else {
        return Ok(None);
    };
    let genv = heap.global();
    let Some(Value::Map(impls)) = heap.env_get(genv, intern("*impls*")) else {
        return Ok(None);
    };
    let key = heap.alloc_vector2(Value::Sym(intern("Counted")), Value::Sym(intern("->count")));
    let Some(Value::Map(methods)) = heap.map_get(impls, key) else {
        return Ok(None);
    };
    let Some(method) = heap.map_get(methods, id) else {
        return Ok(None);
    };
    Ok(Some(crate::eval::compile::apply_engine(
        heap,
        method,
        &[v],
        genv,
    )?))
}

pub(crate) fn is_empty(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let x0 = arg(args, 0);
    if let Some(redirect) = record_redirect(heap, args, "%record-empty?", x0) {
        return Err(redirect);
    }
    // A record that knows its size (`Counted`) is empty iff that size is zero.
    if let Some(n) = record_counted(heap, x0)? {
        return Ok(Value::boolean(matches!(n, Value::Int(0))));
    }
    // a record is empty iff its `Seqable` view is (a custom empty queue, a field-less
    // record) — not iff the raw map is (which always carries `:__id__`).
    let x = match record_seq(heap, x0)? {
        Some(s) => s,
        None => x0,
    };
    match x {
        Value::Nil => Ok(Value::boolean(true)),
        Value::Pair(_) | Value::Range(_) => Ok(Value::boolean(false)),
        Value::SeqView(_) => {
            let realized = realize_seqview(heap, env, x)?;
            Ok(Value::boolean(matches!(realized, Value::Nil)))
        }
        Value::Str(id) => Ok(Value::boolean(heap.string(id).is_empty())),
        Value::Vector(id) => Ok(Value::boolean(heap.vector(id).is_empty())),
        Value::Bytes(id) => Ok(Value::boolean(heap.bytes(id).as_bytes().is_empty())),
        Value::Map(id) => Ok(Value::boolean(heap.map_size(id) == 0)),
        Value::Set(id) => Ok(Value::boolean(heap.map_size(id) == 0)),
        // A rope and a table are collections with an O(1) size, and both answered
        // `empty?: expected collection` (ADR-253). They are sized here rather than
        // routed through `Seqable`: `->seq` is a LIST view, so a rope would have to
        // materialise every character to answer a question its length already
        // answers — and a rope is on the editor's hot path. A rope sizes in
        // characters, which is what `count` and `string/length` already report for
        // the string it stands for.
        Value::Rope(_) => {
            let r = expect_rope_ref(heap, "empty?", x)?;
            Ok(Value::boolean(r.len_chars() == 0))
        }
        Value::Table(_) => {
            let id = super::table::expect_table(heap, "empty?", x)?;
            Ok(Value::boolean(crate::core::table::count(id)? == 0))
        }
        _ => Err(LispError::wrong_type(heap, "empty?", "collection", x)),
    }
}

/// `(%range lo hi step)` — construct a lazy integer range. Returns `Nil` for an
/// empty range; errors on a zero step. The arg-parsing arities live in the
/// Brood `range`, which calls this with all three resolved.
pub(super) fn range_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let lo = expect_int(heap, "%range", arg(args, 0))?;
    let hi = expect_int(heap, "%range", arg(args, 1))?;
    let step = expect_int(heap, "%range", arg(args, 2))?;
    if step == 0 {
        return Err(LispError::runtime("range: step must be non-zero")
            .with_hint("use a positive or negative step, e.g. (range 0 10 2)"));
    }
    Ok(heap.alloc_range(lo, hi, step))
}

/// `(range? x)` — true iff `x` is a lazy range handle. (Empty ranges are `Nil`,
/// so this is false for them — the empty case takes the ordinary list path.)
pub(super) fn range_pred(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(arg(args, 0), Value::Range(_))))
}

/// `(%range-bounds rng)` — `[lo hi step]`, for the compiler's counted-loop rewrite of a
/// fold over a range (ADR-360 §7); it reads them once, then loops without the range.
pub(super) fn range_bounds(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => {
            let (lo, hi, step) = heap.range_parts(id);
            Ok(heap.alloc_vector(vec![Value::int(lo), Value::int(hi), Value::int(step)]))
        }
        v => Err(LispError::wrong_type(heap, "%range-bounds", "range", v)),
    }
}

/// `(%range-count rng)` — the element count of a range, O(1).
pub(super) fn range_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => Ok(Value::int(heap.range_len(id))),
        Value::Nil => Ok(Value::int(0)),
        v => Err(LispError::wrong_type(heap, "%range-count", "range", v)),
    }
}

/// `(%range->list rng)` — realise a range to a concrete list (the slow path
/// behind `seq`/`reverse`/`nth` on a range).
pub(super) fn range_to_list(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => {
            // Fallible since the pre-size `capacity overflow` panic fix: a range wider
            // than `MAX_REALISED_RANGE` is refused as a catchable error, not a panic.
            let items = heap.range_to_vec(id)?;
            Ok(heap.list(items))
        }
        Value::Nil => Ok(Value::nil()),
        v => Err(LispError::wrong_type(heap, "%range->list", "range", v)),
    }
}

/// `(%seqview source xform)` — construct a lazy seq-view over `source` carrying
/// the transducer `xform`. The prelude `map`/`filter`/`keep`/`remove` build these
/// (composing `xform` when `source` is already a view); `fold`/`seq` fuse or
/// realise them.
pub(super) fn seqview_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let source = arg(args, 0);
    let xform = arg(args, 1);
    Ok(heap.alloc_seqview(source, xform))
}

/// `(%seqview-parts sv)` — the view's `[source xform]` as a 2-element vector, for
/// the prelude to fuse `fold` over the source or realise via the transducer.
pub(super) fn seqview_parts(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::SeqView(id) => {
            let (source, xform) = heap.seqview_parts(id);
            Ok(heap.alloc_vector2(source, xform))
        }
        v => Err(LispError::wrong_type(heap, "%seqview-parts", "seq-view", v)),
    }
}

/// `(seqview? x)` — is `x` a lazy seq-view (a `map`/`filter`/… result not yet
/// realised)? The fold-family fast-path predicate, mirroring `range?`.
pub(super) fn seqview_pred(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(matches!(arg(args, 0), Value::SeqView(_))))
}

/// Realise a lazy seq-view to a concrete list. The realisation runs the view's
/// transducer over its source, which means applying a Brood closure — so it is
/// delegated to the prelude `%seqview-realize` (`(reverse (fold %flip-cons nil
/// sv))`, which fuses through `fold`'s seq-view branch). Resolved against the
/// live global env so a user redefinition is honoured. The kernel uses this from
/// the hot `first`/`rest` builtins; every other consumer realises in the prelude
/// (via `seq`) or fuses (via `fold`).

// ---- The native folds behind `fold` (ADR-403) ----
//
// `fold` walks a vector or a range in spans: each `%…-fold-span` call folds up to `n`
// elements in a native loop and returns the accumulator when the collection is done — most
// folds are one span, so the common case allocates nothing — or, when the span stopped short,
// `(:%fold-more acc . next)`, `next` where to go on (only a reducer returning a list headed by
// that internal keyword on purpose could pass for one). The walk around them is Brood
// (`std/prelude/core.blsp`), so the process yields at its back edge like any loop's. A span
// ends early, between two elements, when the quantum owes a preemption that a reducer's own
// run could not pay (`stop_between_elements`): so a fold yields within one reducer call of
// its quantum ending. A primitive reducer (`+`, `*`), inlined here, runs the whole span and charges the
// quantum for it (one reduction per `ELEMENTS_PER_REDUCTION`).
//
// A span folds ONLY such a reducer (ADR-502). Any other answers `:%fold-walk` before an
// element is folded, and the prelude walks the collection in Brood, calling the reducer from
// the process's own engine: a call from here would run it under this native frame, which no
// engine can capture, so a `receive` in it blocked the worker instead of parking.

const ELEMENTS_PER_REDUCTION: usize = 8;

/// Charge the quantum for `elements` folded by an inlined primitive step.
fn charge_folded(elements: usize) {
    crate::process::charge_reductions((elements / ELEMENTS_PER_REDUCTION) as u32);
}

/// The tag of a span that stopped short — see the section comment.
static FOLD_MORE: LazyLock<Symbol> = LazyLock::new(|| value::intern("%fold-more"));

/// What a span answers for a reducer it does not inline, before folding anything: the
/// prelude's walk calls that reducer itself (ADR-502, see [`Reducer`]).
static FOLD_WALK: LazyLock<Symbol> = LazyLock::new(|| value::intern("%fold-walk"));

/// A span's result: `acc` when the collection is done, else `(:%fold-more acc . next)`.
/// Allocation never collects, so `acc` needs no root across the two cells.
fn span_result(heap: &mut Heap, acc: Value, next: Option<i64>) -> LispResult {
    let Some(next) = next else {
        return Ok(acc);
    };
    let rest = heap.alloc_pair(acc, Value::int(next));
    Ok(heap.alloc_pair(Value::keyword(*FOLD_MORE), rest))
}

/// A span's reducer: a primitive it inlines (`+`, `*`), resolved once per span. A span folds
/// nothing else (ADR-502): any other reducer is Brood to call, and a call from this native
/// loop would run under its Rust frame, where a `receive` in the reducer — or an interrupt
/// window it opens — blocks the worker instead of parking the process. So the span declines
/// it ([`FOLD_WALK`]) and the prelude's walk calls it from the process's own engine.
struct Reducer {
    prim: crate::eval::compile::PrimOp,
    use_vm: bool,
}

impl Reducer {
    fn resolve(heap: &Heap, f: Value) -> Option<Reducer> {
        let prim = crate::eval::compile::reduce_prim_op(heap, f)?;
        let use_vm = crate::eval::compile::tier_ceiling() >= crate::eval::compile::Tier::Bytecode;
        Some(Reducer { prim, use_vm })
    }

    /// One step: the inlined primitive when it covers these operands (`true`), else a call of
    /// `f` — a bignum, a ratio, a record's `Num` method — through the active engine.
    fn step(
        &self,
        heap: &mut Heap,
        f: Value,
        acc: Value,
        x: Value,
        env: EnvId,
    ) -> Result<(Value, bool), LispError> {
        if let Some(v) = crate::eval::compile::prim_apply(self.prim, acc, x)? {
            return Ok((v, true));
        }
        let v = if self.use_vm {
            crate::eval::compile::apply_value(heap, f, &[acc, x], env)?
        } else {
            apply(heap, f, &[acc, x], env)?
        };
        Ok((v, false))
    }
}

/// `(%vector-fold-span f acc v from n)` — fold up to `n` elements of `v` from index `from`
/// (nil: the start) with `(f acc item)`: the accumulator, or `(:%fold-more acc . next)` with `next`
/// the index to go on from when the span stopped short. See the section comment.
pub(super) fn vector_fold_span(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let vid = match arg(args, 2) {
        Value::Vector(id) => id,
        Value::Nil => return Ok(init),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%vector-fold-span",
                "vector",
                v,
            ));
        }
    };
    let len = heap.vector(vid).len();
    let start = arg(args, 3)
        .as_int()
        .map_or(0, |i| (i.max(0) as usize).min(len));
    let n = arg(args, 4).as_int().map_or(len, |k| k.max(1) as usize);
    let end = start.saturating_add(n).min(len);
    let Some(reducer) = Reducer::resolve(heap, f) else {
        return Ok(Value::keyword(*FOLD_WALK));
    };
    let mut i = start;
    let mut acc = init;
    // Tight i64 loop: no Value boxing, no root slot per element (integers are inline). It
    // stops at a non-int element or an overflow, and the boxed loop goes on from there, so a
    // BigInt promotion stays bit-identical to the Brood fold.
    if let Some(mut int_acc) = init.as_int() {
        let op = reducer.prim;
        while i < end {
            let Some(x) = heap.vector(vid)[i].as_int() else {
                break;
            };
            match crate::eval::compile::prim_apply_int_step(op, int_acc, x) {
                Some(v) => int_acc = v,
                None => break,
            }
            i += 1;
        }
        charge_folded(i - start);
        acc = Value::int(int_acc);
    }
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let v_r = heap.root(Value::Vector(vid));
        let mut acc_r = heap.root(acc);
        while i < end {
            // Re-read through the roots: an apply below may have collected.
            let x = match heap.read_root(v_r) {
                Value::Vector(id) => heap.vector(id)[i],
                _ => break,
            };
            let (f, acc) = (heap.read_root(f_r), heap.read_root(acc_r));
            let (next, inlined) = reducer.step(heap, f, acc, x, env)?;
            acc_r = heap.advance_root(acc_r, next);
            i += 1;
            if !inlined && i < end && crate::process::stop_between_elements() {
                break;
            }
        }
        let acc = heap.read_root(acc_r);
        span_result(heap, acc, (i < len).then_some(i as i64))
    })
}

/// The i64 loop of [`range_fold_span`]: fold elements from `*i` while `before(*i, end)`,
/// leaving `*i` at the first element not folded (an overflow of the accumulator stops it
/// there, for the boxed loop to go on from; the last element of an i64-edge range leaves it
/// at `end`). Returns how many it folded.
#[inline(always)]
fn fold_ints(
    op: crate::eval::compile::PrimOp,
    acc: &mut i64,
    i: &mut i64,
    step: i64,
    end: i64,
    before: impl Fn(i64, i64) -> bool,
) -> usize {
    // Locals, not the references, through the loop; the count is derived after it, so an
    // element costs its step and its compare.
    let (start, mut a, mut x) = (*i, *acc, *i);
    while before(x, end) {
        match crate::eval::compile::prim_apply_int_step(op, a, x) {
            Some(v) => a = v,
            None => break,
        }
        match x.checked_add(step) {
            Some(v) => x = v,
            None => {
                x = end;
                break;
            }
        }
    }
    *acc = a;
    *i = x;
    (x.abs_diff(start) / step.unsigned_abs().max(1)) as usize
}

/// `(%range-fold-span f acc rng from n)` — fold up to `n` elements of a range from the element
/// `from` (nil: the start) with `(f acc item)`, **without materialising** it: the accumulator,
/// or `(:%fold-more acc . next)` with `next` the element to go on from when the span stopped
/// short. See the section comment.
pub(super) fn range_fold_span(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let (lo, hi, step) = match arg(args, 2) {
        Value::Range(id) => heap.range_parts(id),
        Value::Nil => return Ok(init), // empty range
        v => return Err(LispError::wrong_type(heap, "%range-fold-span", "range", v)),
    };
    let from = arg(args, 3).as_int().unwrap_or(lo);
    let in_range = |i: i64| if step > 0 { i < hi } else { i > hi };
    // The span ends `n` elements on, or at the range's end, whichever is first — the bound the
    // loops below compare against, so an element costs the one compare it always did.
    let n = arg(args, 4).as_int().unwrap_or(i64::MAX).max(1);
    let span_end = from.saturating_add(step.saturating_mul(n));
    let span_end = if step > 0 {
        span_end.min(hi)
    } else {
        span_end.max(hi)
    };
    let before_end = |i: i64| if step > 0 { i < span_end } else { i > span_end };
    let Some(reducer) = Reducer::resolve(heap, f) else {
        return Ok(Value::keyword(*FOLD_WALK));
    };
    let mut i = from;
    let mut acc = init;
    if let Some(mut int_acc) = init.as_int() {
        let op = reducer.prim;
        // One loop per direction, so the per-element bound is a plain compare.
        let folded = if step > 0 {
            fold_ints(op, &mut int_acc, &mut i, step, span_end, |i, end| i < end)
        } else {
            fold_ints(op, &mut int_acc, &mut i, step, span_end, |i, end| i > end)
        };
        charge_folded(folded);
        acc = Value::int(int_acc);
    }
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let mut acc_r = heap.root(acc);
        let mut past_end = false;
        while !past_end && before_end(i) {
            let (f, acc) = (heap.read_root(f_r), heap.read_root(acc_r));
            let (next, inlined) = reducer.step(heap, f, acc, Value::int(i), env)?;
            acc_r = heap.advance_root(acc_r, next);
            match i.checked_add(step) {
                Some(v) => i = v,
                None => past_end = true,
            }
            if !inlined && !past_end && before_end(i) && crate::process::stop_between_elements() {
                break;
            }
        }
        let acc = heap.read_root(acc_r);
        span_result(heap, acc, (!past_end && in_range(i)).then_some(i))
    })
}

/// `(%sort-asc coll)` — stable ascending sort of a numeric collection by `<`.
/// The fast path behind `(sort coll)` when no custom comparator is given;
/// the all-Brood `%merge-sort` in `std/prelude.blsp` still handles
/// `(sort coll less?)`. ~50× faster than the in-Brood mergesort on 10 000
/// items because every comparison is a Rust `match` instead of an
/// `eval::apply` round-trip.
///
/// Items must be `Int` / `Float` / mixed (the same shape `<` accepts).
/// Mixed Int+Float promote to float for the compare (matching `prim_lt`).
/// Any non-numeric item is a `wrong_type` error against the offending value.
pub(super) fn sort_asc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Collect into a Vec. `seq_items` walks the cons spine (or copies a
    // vector) once. Values are `Copy` so the Vec holds plain handles — no
    // GC root machinery needed because `sort_by` does no eval and can't
    // trigger a safepoint.
    let mut items = heap.seq_items(arg(args, 0))?;

    // Validate before sorting so a non-numeric item produces one clear
    // error rather than an indeterminate-order partial sort. The same pass
    // unboxes the all-`Int` case into a plain `Vec<i64>` — see below for why
    // that is worth a second buffer.
    let mut ints: Vec<i64> = Vec::with_capacity(items.len());
    let mut all_int = true;
    for &v in &items {
        match v {
            Value::Int(n) => {
                // Once a Float has been seen the i64 buffer is dead, but the
                // loop still has to run to validate the remaining items.
                if all_int {
                    ints.push(n);
                }
            }
            // Any other number (Float/BigInt/Ratio/Decimal) drops the i64 fast
            // path; the general `value_cmp` sort below orders the full tower.
            Value::Float(_) | Value::BigInt(_) | Value::Ratio(_) | Value::Decimal(_) => {
                if all_int {
                    all_int = false;
                    ints = Vec::new(); // release the partial buffer
                }
            }
            _ => return Err(LispError::wrong_type(heap, "sort", "number", v)),
        }
    }

    // All-`Int` fast path: sort raw i64s instead of `Value`s. The general
    // `sort_by` below is a stable merge sort whose comparator re-`match`es a
    // 24-byte enum on every one of the ~n log n comparisons; on a slice of
    // i64 the compiler gets an unboxed, branch-predictable compare it can
    // vectorise.
    //
    // Comparison is only a minor share of `sort` on a large list: the rest is
    // `seq_items` walking the cons spine in and `heap.list` allocating a fresh
    // list out, which this fast path does not touch. Chase the traversal/
    // rebuild (allocation), not the comparator.
    //
    // `sort_unstable` is safe to use here even though the general path is
    // stable: two equal `Int`s are the same value, so no observable ordering
    // distinguishes them. That does NOT hold for the mixed path, where an
    // Int and a Float can compare equal while remaining distinguishable
    // (`1` vs `1.0`), which is why only this branch drops stability.
    if all_int {
        if ints.len() > 2 * SORT_CHUNK {
            // large: chunked so an interrupt can stop it (ADR-491), and the merge of the
            // chunks wants the stable sort's run detection
            sort_interruptibly(&mut ints, i64::cmp)?;
        } else {
            ints.sort_unstable();
        }
        // Reuse the `items` allocation rather than building a second Vec.
        for (slot, n) in items.iter_mut().zip(ints) {
            *slot = Value::Int(n);
        }
        return Ok(heap.list(items));
    }

    // Stable sort over the full numeric tower via the canonical `value_cmp`
    // (exact for Int/BigInt/Ratio/Decimal; Int-vs-Float compares precisely in
    // base 10). Only reached once a non-`Int` number appeared, so the common
    // all-int case above never pays for it.
    sort_interruptibly(&mut items, |a, b| heap.value_cmp(*a, *b))?;

    Ok(heap.list(items))
}

/// `(%sort-cmp coll)` — stable ascending sort by the structural total order
/// (`Heap::value_cmp`). The Brood `sort` (prelude) routes here when items
/// aren't all numeric, so `(sort [[1 0] [2 1]])` and similar work without a
/// custom comparator. Cross-kind items get a defined tag-rank order rather
/// than the old "expected number" trap.
pub(super) fn sort_cmp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut items = heap.seq_items(arg(args, 0))?;
    // `value_cmp` reads heap data through `&Heap` only; the items are `Copy`
    // handles, so no GC root machinery is needed.
    sort_interruptibly(&mut items, |a, b| heap.value_cmp(*a, *b))?;
    Ok(heap.list(items))
}

/// A stable sort of `items` by `compare` that an interrupt can stop (ADR-491). A large input
/// is sorted in chunks of [`SORT_CHUNK`] with a probe between them, then the whole once
/// more: the standard sort finds the sorted chunks as runs and only merges them, a few linear
/// passes. No comparison pays anything for the probe; a small input is one plain sort.
fn sort_interruptibly<T>(
    items: &mut [T],
    mut compare: impl FnMut(&T, &T) -> std::cmp::Ordering,
) -> Result<(), LispError> {
    if items.len() > 2 * SORT_CHUNK {
        for chunk in items.chunks_mut(SORT_CHUNK) {
            crate::eval::check_interrupt()?;
            chunk.sort_by(&mut compare);
        }
        crate::eval::check_interrupt()?;
    }
    items.sort_by(compare);
    Ok(())
}

/// Items per chunk of an interruptible sort: a few milliseconds of comparisons at most.
const SORT_CHUNK: usize = 1 << 16;

/// `(compare a b)` — the structural total order as a binary comparison: `-1` if
/// `a` sorts before `b`, `0` if equal, `1` if after. Numbers compare
/// numerically; strings/keywords/symbols by text; vectors/lists
/// lexicographically; cross-kind values by a stable tag rank. The binary form of
/// the order `sort` uses, so `sort-by` and custom comparators work over any
/// orderable value, not just numbers.
pub(super) fn compare(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::cmp::Ordering;
    let ord = match heap.value_cmp(arg(args, 0), arg(args, 1)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    Ok(Value::int(ord))
}

pub(super) fn hash(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::int(heap.hash_value(arg(args, 0)) as i64))
}

// ---------- vector ----------

pub(super) fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector_from_slice(args))
}

pub(super) fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let n = expect_int(heap, "%vector-ref", arg(args, 1))?;
    match v {
        Value::Vector(id) if n >= 0 && (n as usize) < heap.vector(id).len() => {
            Ok(heap.vector(id)[n as usize])
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "%vector-ref: index {} out of range [0, {})",
            n,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "%vector-ref", "vector", v)),
    }
}

pub(super) fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Vector(id) => Ok(Value::int(heap.vector(id).len() as i64)),
        _ => Err(LispError::wrong_type(heap, "%vector-length", "vector", v)),
    }
}

/// `(vector-assoc v i x)` — a fresh vector like `v` with index `i` set to `x`.
/// The vector counterpart of `%map-assoc`; O(n) copy (vectors are flat), one
/// allocation, no cons churn. `i` must be in `[0, len)` (append-at-end is a
/// deferred power feature, ADR-011). No GC safepoint runs inside a builtin, so
/// the cloned handles stay valid across `alloc_vector`.
pub(super) fn vector_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let i = expect_int(heap, "%vector-assoc", arg(args, 1))?;
    let x = arg(args, 2);
    match v {
        Value::Vector(id) if i >= 0 && (i as usize) < heap.vector(id).len() => {
            let mut items = heap.vector(id).to_vec();
            items[i as usize] = x;
            Ok(heap.alloc_vector(items))
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "%vector-assoc: index {} out of range [0, {})",
            i,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "%vector-assoc", "vector", v)),
    }
}

/// `(%vector-concat v xs)` — a fresh vector of `v`'s items followed by `xs`'s (a
/// vector, a list, a range, a set or nil). The kernel op behind `conj` on a vector and
/// `into` onto one, rather than building a list, reversing it twice and re-vectorising it in
/// code every game frame runs. It appends in place when `v` is the newest vector over an
/// append buffer, so a loop of `conj` is amortised O(1) a step rather than a copy of
/// everything so far (ADR-511, `Heap::vector_append`). No GC safepoint runs inside a
/// builtin, so the handles stay valid.
pub(super) fn vector_concat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let id = match v {
        Value::Vector(id) => id,
        _ => return Err(LispError::wrong_type(heap, "%vector-concat", "vector", v)),
    };
    let tail = heap.seq_items(arg(args, 1))?;
    Ok(heap.vector_append(id, &tail))
}

/// `(subvec v start)` / `(subvec v start end)` — a fresh vector of the elements
/// of `v` in `[start, end)` (`end` defaults to the length). `0 <= start <= end
/// <= len`; out of range is an error. The slice counterpart of `substring`, and
/// the vector-preserving slice the list-returning `take`/`drop` don't give.
pub(super) fn subvec(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let id = match v {
        Value::Vector(id) => id,
        _ => return Err(LispError::wrong_type(heap, "seq/subvec", "vector", v)),
    };
    let len = heap.vector(id).len() as i64;
    let start = expect_int(heap, "seq/subvec", arg(args, 1))?;
    let end = if args.len() > 2 {
        expect_int(heap, "seq/subvec", arg(args, 2))?
    } else {
        len
    };
    if start < 0 || end > len || start > end {
        return Err(LispError::runtime(format!(
            "seq/subvec: range [{start}, {end}) out of bounds for vector of length {len}"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let items = heap.vector(id)[start as usize..end as usize].to_vec();
    Ok(heap.alloc_vector(items))
}

// ---------- map ----------

/// `(hash-map k v k v …)` — build a map from alternating key/value args (the
/// programmatic form of the `{ }` literal). Errors on an odd count; last-wins on
/// duplicate keys.
pub(super) fn hash_map(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if !args.len().is_multiple_of(2) {
        return Err(LispError::arity(
            "hash-map: expected an even number of arguments (key/value pairs)",
        ));
    }
    let pairs: Vec<(Value, Value)> = args
        .as_chunks::<2>()
        .0
        .iter()
        .map(|kv| (kv[0], kv[1]))
        .collect();
    Ok(heap.map_from_pairs(pairs))
}

/// The `[k v]` of a pair item — a `[k v]` vector or a `(k v)` list of exactly two
/// items; anything else is a type error naming the entry. Used by
/// [`map_into`] to read the items of an `into`/`zipmap` sequence.
pub(super) fn pair_kv(heap: &Heap, who: &str, p: Value) -> Result<(Value, Value), LispError> {
    match p {
        // Exactly two items: a one-item entry must not read as `[k nil]`, nor a
        // three-item one silently drop its tail.
        Value::Vector(id) => {
            let v = heap.vector(id);
            if v.len() == 2 {
                return Ok((v[0], v[1]));
            }
        }
        Value::Pair(id) => {
            let (k, rest) = heap.pair(id);
            if let Value::Pair(rid) = rest {
                let (val, tail) = heap.pair(rid);
                if matches!(tail, Value::Nil) {
                    return Ok((k, val));
                }
            }
        }
        _ => {}
    }
    Err(LispError::wrong_type(heap, who, "a [key value] entry", p))
}

/// `(%map-into m seq)` — pour each `[k v]` item of `seq` into map `m`, returning
/// a fresh map, via the transient builder (`Heap::map_from_pairs_into`, see
/// `docs/transients.md`). The kernel hook behind the prelude's `into` (map
/// branch), `zipmap`, and `select-keys`; equals `(reduce assoc m seq)` but
/// mutates only build-local trie nodes, so it allocates O(result-nodes) rather
/// than O(n·depth).
pub(super) fn map_into(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let into = expect_map(heap, "%map-into", arg(args, 0))?;
    let items = heap.seq_items(arg(args, 1))?;
    let mut pairs = Vec::with_capacity(items.len());
    for it in items {
        pairs.push(pair_kv(heap, "%map-into", it)?);
    }
    Ok(heap.map_from_pairs_into(into, pairs))
}

/// `(%dispatch impls op-key id)` — ability dispatch through the per-op inline cache
/// (ADR-172 §7). `impls` is the `*impls*` registry (passed by the op so the kernel stays
/// decoupled from the global's name), `op-key` the constant `[ability op]` vector, `id`
/// the first argument's dispatch keyword. Returns the impl `fn` (or nil). A pure,
/// cache-transparent memo of `impl-for`; see [`Heap::vm_dispatch`].
pub(super) fn dispatch(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.vm_dispatch(arg(args, 0), arg(args, 1), arg(args, 2)))
}

/// `(%dispatch-proven op proven-op impls op-key id [ctor proven-ctor])` — the head a
/// devirtualized ability call computes (`inline::mono_devirtualize`): the impl for the
/// compile-time-proven `id`, or `op` itself — its CURRENT binding — whenever the proof no
/// longer holds, so the call then runs the op's own dispatch and answers exactly what the
/// dynamic call would. The proof holds while `op` is still the closure the compiler saw and,
/// for a constructor receiver, `ctor` is still the constructor whose `:__id__` is `id`. A
/// missing impl also answers `op`, so the op raises its own `%no-impl` error.
pub(super) fn dispatch_proven(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let same = |a: Value, b: Value| matches!((a, b), (Value::Fn(x), Value::Fn(y)) if x == y);
    let op = arg(args, 0);
    if !same(op, arg(args, 1)) || (args.len() == 7 && !same(arg(args, 5), arg(args, 6))) {
        return Ok(op);
    }
    match heap.vm_dispatch(arg(args, 2), arg(args, 3), arg(args, 4)) {
        Value::Nil => Ok(op),
        resolved => Ok(resolved),
    }
}

/// `(%registry-update! name op path value)` — atomically read-modify-write a global that
/// holds a registry (KI-22). `name` is the global's symbol, `op` one of `:assoc`,
/// `:assoc-new`, `:dissoc`, `:cons-new`, `path` a vector of one or two keys (nil for
/// `:cons-new`). Returns true if the registry was written, false if the op declined.
///
/// The whole sequence runs inside [`Heap::registry_update`] under the runtime's registry
/// lock — see there for why `(def *X* (assoc *X* …))` in Brood cannot be made safe.
pub(super) fn registry_update(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    use crate::core::heap::RegistryOp;
    let sym = expect_symbol(heap, "%registry-update!", arg(args, 0))?;
    // Keywords are interned, so compare the interned ids rather than the Values.
    let op_sym = match arg(args, 1) {
        Value::Keyword(k) => k,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-update!",
                "keyword",
                v,
            ));
        }
    };
    let op = match value::symbol_name_ref(op_sym) {
        "assoc" => RegistryOp::Assoc,
        "assoc-new" => RegistryOp::AssocNew,
        "dissoc" => RegistryOp::Dissoc,
        "cons-new" => RegistryOp::ConsNew,
        "append-new" => RegistryOp::AppendNew,
        "merge" => RegistryOp::Merge,
        _ => {
            return Err(LispError::type_err(
                "%registry-update!: op must be :assoc, :assoc-new, :dissoc, :cons-new, :append-new or :merge",
            ));
        }
    };
    let path = match arg(args, 2).unpack() {
        crate::core::value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => Vec::new(),
    };
    let from = match args.get(4).copied().unwrap_or(Value::Nil) {
        Value::Nil => None,
        Value::Sym(s) => Some(s),
        // A module KEY as the image replay holds it (a string): the same name, interned.
        Value::Str(id) => Some(value::intern(heap.string(id))),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-update!",
                "symbol",
                v,
            ));
        }
    };
    Ok(Value::boolean(heap.registry_update(
        env,
        sym,
        op,
        &path,
        arg(args, 3),
        from,
    )))
}

/// `(%registry-writer name path)` — the module whose load wrote registry `name`'s entry at
/// `path` (a vector of one or two keys), or nil for an entry nobody owns. See
/// [`Heap::registry_writer`] (KI-136).
pub(super) fn registry_writer(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%registry-writer", arg(args, 0))?;
    let path = match arg(args, 1).unpack() {
        crate::core::value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => Vec::new(),
    };
    Ok(heap
        .registry_writer(sym, &path)
        .map(Value::Sym)
        .unwrap_or(Value::Nil))
}

/// `(%registry-member? name key)` — is registry global `name` a map containing `key`, read
/// from the shared globals table bypassing the per-process inline cache (ADR-225)? For a
/// load-once guard that must not miss a racing `provide`; see [`Heap::registry_member`].
pub(super) fn registry_member(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%registry-member?", arg(args, 0))?;
    Ok(Value::boolean(heap.registry_member(sym, arg(args, 1))))
}

/// `(%registry-cas! name old new)` — compare-and-swap a registry global (KI-23). Rebinds
/// `name` to `new` and returns true only if its current value still equals `old`; returns
/// false otherwise, leaving it untouched, so the caller can recompute and retry.
///
/// The general form of `%registry-update!`, for a registry whose update is not one map/list
/// op — `face-set`'s merge into the existing entry, `attach`'s strip-then-cons, the REPL's
/// filter-then-append. The transform stays an ordinary Brood function (the prelude's
/// `registry-swap!` retries around this); only the read-decide-write is indivisible. See
/// [`Heap::registry_cas`].
pub(super) fn registry_cas(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%registry-cas!", arg(args, 0))?;
    Ok(Value::boolean(heap.registry_cas(
        env,
        sym,
        arg(args, 1),
        arg(args, 2),
    )))
}

/// `(%registry-names)` — the symbols of every global a registry update has written in this
/// runtime, as a list. See [`Heap::registry_names`]: it is what lets `std/tool/project.blsp`
/// derive the startup image's registry set instead of naming it (ADR-218).
pub(super) fn registry_names(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = heap.registry_names().into_iter().map(Value::Sym).collect();
    Ok(heap.list(names))
}

/// `(%map-get m k [default])` — the value `k` maps to, or `default` (nil if
/// omitted) when absent.
pub(super) fn map_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-get", arg(args, 0))?;
    Ok(heap
        .map_get(id, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(%map-assoc m k v)` — a fresh map with `k` bound to `v`.
pub(super) fn map_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-assoc", arg(args, 0))?;
    Ok(heap.map_assoc(id, arg(args, 1), arg(args, 2)))
}

/// `(%map-int-add m k delta)` — a fresh map with `k`'s value incremented by the
/// integer `delta` (inserts `delta` when `k` is absent): exactly
/// `(assoc m k (+ (get m k 0) delta))`, in a single trie traversal when the stored
/// value is a plain integer. The linear-map optimizer rewrites this — and the
/// idiomatic spelling — into `%table-add`, which is the same `+`, so the two agree
/// on every input (see [`Heap::map_int_add`]).
pub(super) fn map_int_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-int-add", arg(args, 0))?;
    let delta = expect_int(heap, "%map-int-add", arg(args, 2))?;
    heap.map_int_add(id, arg(args, 1), delta)
}

/// `(%map-dissoc m k)` — a fresh map with `k` removed.
pub(super) fn map_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-dissoc", arg(args, 0))?;
    Ok(heap.map_dissoc(id, arg(args, 1)))
}

/// `(%map-pairs m)` — the entries as a list of `[k v]` vectors, in the trie's hash-derived
/// order (not insertion order), in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`map/reduce-kv` are all Brood over it (std/prelude/map.blsp, std/map.blsp).
pub(super) fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-pairs", arg(args, 0))?;
    let entries = heap.map_entries(id); // copy out, releasing the borrow before we alloc
    Ok(entry_pair_list(heap, &entries))
}

/// A list of `[k v]` vectors over `entries`, in their order. Built back to front so each
/// pair conses straight onto the list — no second `Vec` of the vectors to hand to
/// `Heap::list` — and each 2-vector is an inline slab entry ([`Heap::alloc_vector2`]), not
/// a `vec!` buffer copied in and freed. Allocation never collects inside a native, so the
/// handles built so far stay valid.
fn entry_pair_list(heap: &mut Heap, entries: &[(Value, Value)]) -> Value {
    let mut list = Value::nil();
    for &(key, value) in entries.iter().rev() {
        let pair = heap.alloc_vector2(key, value);
        list = heap.alloc_pair(pair, list);
    }
    list
}

/// `(%map-count m)` — the number of entries, O(1). The CHAMP root node tracks
/// its subtree size, so this never walks (or allocates) the entries; it's what
/// `count`/`empty?` on a map use instead of materialising `%map-pairs`.
pub(super) fn map_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-count", arg(args, 0))?;
    Ok(Value::int(heap.map_size(id) as i64))
}

/// Re-wrap the `MapId` a map op just produced as a **set** (both share the CHAMP
/// store; the set-op natives keep the backing values all `true`).
fn as_set(v: Value) -> Value {
    match v {
        Value::Map(id) => Value::set(id),
        _ => unreachable!("map op returns Value::Map"),
    }
}

/// `(%set a b c …)` — build a set from element args (the programmatic form of the
/// `#{ }` literal). Dedups by structural equality; every op returns a fresh set.
pub(super) fn set_construct(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.set_from_elem_iter(args.iter().copied()))
}

/// `(%set-add s x)` — a fresh set with `x` added (a set already holding `x` is
/// returned structurally unchanged — the CHAMP `assoc` is a no-op).
pub(super) fn set_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-add", arg(args, 0))?;
    Ok(as_set(heap.map_assoc(id, arg(args, 1), Value::Bool(true))))
}

/// `(%set-remove s x)` — a fresh set with `x` removed (absent → unchanged).
pub(super) fn set_remove(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-remove", arg(args, 0))?;
    Ok(as_set(heap.map_dissoc(id, arg(args, 1))))
}

/// `(%set-has? s x)` — is `x` an element of set `s`? O(log n) trie lookup.
pub(super) fn set_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-has?", arg(args, 0))?;
    Ok(Value::boolean(heap.map_get(id, arg(args, 1)).is_some()))
}

/// `(%set-count s)` — the number of elements, O(1) (the CHAMP root tracks size).
pub(super) fn set_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_set(heap, "%set-count", arg(args, 0))?;
    Ok(Value::int(heap.map_size(id) as i64))
}

pub(super) fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        // O(1): the char count is cached on the slot, so a loop bounded by
        // `(string/length s)` is not quadratic before it does anything else.
        Value::Str(id) => Ok(Value::int(heap.str_metrics(id).0 as i64)),
        _ => Err(LispError::wrong_type(heap, "string/length", "string", v)),
    }
}

/// The display width of a string in terminal/grid cells, counted in grapheme
/// clusters (an emoji / flag / CJK char is 2, a combining mark 0, a tab the cells to
/// its next stop). The width-aware counterpart to `string-length` (which counts
/// codepoints) — the editor's column / cursor math uses it so a wide glyph advances
/// two columns. The GUI renderer advances the cell grid by the same measure
/// (`crate::host::text_width`). The optional `start-col` / `tab-width` are the tab
/// layout (`tab_layout`).
pub(super) fn display_width(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => {
            let (start_col, tab_width) = tab_layout(heap, "string/display-width", args, 1)?;
            Ok(Value::int(crate::host::text_width::display_width_from(
                heap.string(id),
                start_col,
                tab_width,
            ) as i64))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "string/display-width",
            "string",
            v,
        )),
    }
}

/// The tab width a cell-layout call with no `tab-width` argument lays out with: the
/// prelude's dynamic `*tab-width*` (ADR-537), read through the process's binding stack
/// so an editor's `(binding (*tab-width* 4) …)` around a pane's render reaches every
/// measure inside it — `string/clip-cells` and the rest of std included — without
/// threading a width through each call. `None` when it is unbound or not an int (a
/// heap built without the prelude), which falls back to `TAB_WIDTH`. A value below 1 is
/// 1, as an argument is.
fn ambient_tab_width(heap: &Heap) -> Option<i64> {
    static TAB_WIDTH_SYMBOL: LazyLock<Symbol> = LazyLock::new(|| value::intern("*tab-width*"));
    match heap.env_get(EnvId::GLOBAL, *TAB_WIDTH_SYMBOL) {
        Some(Value::Int(n)) => Some(n.max(1)),
        _ => None,
    }
}

/// The optional `start-col` and `tab-width` tail of the cell-layout primitives
/// (`string/display-width`, `string/width->index`, `string/expand-tabs`), starting at
/// argument `at`: each an int when supplied, else 0 and the ambient `*tab-width*`
/// ([`ambient_tab_width`], `TAB_WIDTH` = 8 by default). A negative
/// column is 0; a tab width below 1 is 1 (a stop every column). A column past
/// `MAX_START_COLUMN` or a tab width past `MAX_TAB_WIDTH` is an error: the first
/// overflowed the column sum, the second made `string/expand-tabs` allocate without
/// bound and abort the runtime.
pub(super) fn tab_layout(
    heap: &Heap,
    who: &str,
    args: &[Value],
    at: usize,
) -> Result<(usize, usize), LispError> {
    use crate::host::text_width::{MAX_START_COLUMN, MAX_TAB_WIDTH, TAB_WIDTH};
    // Bounded as an `i64` BEFORE the `usize` cast: on a 32-bit target the cast first
    // would truncate 2^33 to 0 and let it through.
    let start_col = if args.len() > at {
        expect_int(heap, who, arg(args, at))?.max(0)
    } else {
        0
    };
    if start_col > MAX_START_COLUMN as i64 {
        return Err(LispError::runtime(format!(
            "{who}: start-col must be at most {MAX_START_COLUMN}, got {start_col}"
        )));
    }
    let start_col = start_col as usize;
    let tab_width = if args.len() > at + 1 {
        expect_int(heap, who, arg(args, at + 1))?.max(1)
    } else {
        ambient_tab_width(heap).unwrap_or(TAB_WIDTH as i64)
    };
    if tab_width > MAX_TAB_WIDTH as i64 {
        return Err(LispError::runtime(format!(
            "{who}: tab-width must be at most {MAX_TAB_WIDTH}, got {tab_width}"
        )));
    }
    Ok((start_col, tab_width as usize))
}

// ---------- type reflection ----------

/// `(type-of x)` — the runtime type tag of `x` as a keyword: `:int` `:float`
/// `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
/// `:macro` `:native`. The single irreducible reflective primitive: the tag
/// predicates (`int?`/`string?`/…) are Brood wrappers over it (`std/prelude.blsp`),
/// and the in-language type checks build on it too.
pub(super) fn type_of(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    // Cached keyword id per tag — `type-of` is hit per element by the seq
    // predicates, so re-interning the tag name here dominated intern cost.
    Ok(Value::keyword(value::tag(arg(args, 0)).keyword()))
}
