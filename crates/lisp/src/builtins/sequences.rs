use super::numeric::{arg, expect_int, expect_rope_ref, two};
use super::realize_seqview;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::apply;

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
        "A new pair with head x and tail xs.\n\n    (cons 1 (list 2))   → (1 2)",
        cons,
    );
    primitives.def(
        "first",
        Arity::exact(1),
        Sig::new(vec![seqable], any),
        &["coll"],
        "The head of any sequence — a list, vector, bytes, set (an element) or map (a [k v] pair) — or nil if empty.\n\n    (first [1 2 3])   → 1\n    (first {:a 1})    → [:a 1]\n    (first [])        → nil",
        first);
    primitives.def(
        "rest",
        Arity::exact(1),
        Sig::new(vec![seqable], list_ty),
        &["coll"],
        "All but the head of any sequence, as a list (a set yields its remaining elements, a map its remaining [k v] pairs). A one-element sequence yields nil, not an empty list.\n\n    (rest [1 2 3])   → (2 3)\n    (rest [1])       → nil",
        rest);
    primitives.def(
        "nil?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "True if x is nil.\n\n    (nil? nil)   → true",
        is_nil,
    );
    primitives.def(
        "pair?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "True if x is a cons pair.\n\n    (pair? (list 1))   → true",
        is_pair,
    );
    primitives.def(
        "empty?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["coll"],
        "True if coll is empty (nil, an empty string/vector/map, or a seq-view that realises to nothing).\n\n    (empty? [])   → true",
        is_empty);
    // Lazy reducible range (ADR: reducible range). `%range` constructs it (arg
    // parsing is in the Brood `range`); the fold-family fast paths in the prelude
    // call `range?` / `%range-reduce` / `%range-count`; everything else realises
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
        "True if x is a lazy range (as produced by range). Ranges fold/reduce/sum/count without materialising; other ops treat them as the list they stand for.\n\n    (range? (range 3))   → true",
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
        "%range->list",
        Arity::exact(1),
        Sig::new(vec![list_ty], list_ty),
        &[],
        "",
        range_to_list,
    );
    primitives.def(
        "%range-reduce",
        Arity::exact(3),
        Sig::new(vec![callable, any, list_ty], any),
        &[],
        "",
        range_reduce,
    );
    // The vector counterpart of `%range-reduce`, behind the prelude `fold`'s vector
    // branch. Same reason: fold the container in a native loop instead of paying a
    // per-element `apply` — and, on this path specifically, resolve a passthrough
    // reducer like `+` once rather than per element.
    primitives.def(
        "%vector-reduce",
        Arity::exact(3),
        Sig::new(vec![callable, any, any], any),
        &[],
        "",
        vector_reduce,
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
        "True if x is a lazy sequence view — the reducible produced by range/map/filter/… before it is realized (into/count/…).\n\n    (seqview? [1 2])   → false",
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
        "Structural total-order comparison: -1 if a sorts before b, 0 if equal, 1 if after. Numbers numerically; strings/keywords/symbols by text; vectors/lists lexicographically; cross-kind by a stable tag rank. The binary form of `sort`'s order — `sort-by` and custom comparators build on it.\n\n    (compare 1 2)   → -1",
        compare);
    // vector
    primitives.def(
        "vector",
        Arity::any(),
        Sig::variadic(any, vec_ty),
        &["&", "items"],
        "A vector of the given items.\n\n    (vector 1 2 3)   → [1 2 3]",
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
        "%subvec",
        Arity::range(2, 3),
        Sig::with_rest(vec![vec_ty, int], int, vec_ty),
        &["v", "start", "end"],
        "A fresh vector of v's elements in [start, end); end defaults to the length.",
        subvec,
    );
    // map — the *minimal* kernel: construct, read, two producers, and one
    // enumerator (`%map-pairs` → [k v] vectors). `keys`/`vals`/`contains?`/
    // `reduce-kv` and the `get`/`assoc`/`dissoc` surface (variadic + defaults) are
    // all Brood over these (std/prelude.blsp). Maps are immutable: each op returns
    // a fresh map.
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
        "A fresh map like m with key k's integer value incremented by delta (inserts delta when k is absent). Single trie traversal — equivalent to (assoc m k (+ (get m k 0) delta)) without the extra walk.",
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
    // Atomic registry update (KI-22): the read-modify-write of a global holding a whole
    // registry map, done in ONE kernel call so two concurrent registrations cannot each
    // read the old map and clobber each other. Internal; `register-impl`/`provide`/
    // `defability`/… in the prelude call it, never user code.
    primitives.def(
        "%registry-update!",
        Arity::exact(4),
        Sig::new(vec![any, any, any, any], any),
        &[],
        "",
        registry_update,
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
        "The number of characters in string s.\n\n    (string/length \"Hi there\")   → 8",
        string_length,
    );
    primitives.def(
        "string/display-width",
        Arity::exact(1),
        Sig::new(vec![string], int),
        &["s"],
        "How many terminal/grid cells string s occupies (grapheme-cluster aware: an emoji / flag / CJK char counts as 2, a combining mark 0). The width-aware counterpart to string/length.\n\n    (string/display-width \"Hi there\")   → 8",
        display_width);
    // type reflection — the tag predicates (nil?/int?/string?/…) are Brood
    // (std/prelude.blsp) over this one reflective primitive.
    primitives.def(
        "type-of",
        Arity::exact(1),
        Sig::new(vec![any], kw),
        &["x"],
        "The runtime type of x as a keyword (:int, :string, :pair, ...).\n\n    (type-of :k)   → :keyword",
        type_of,
    );
}

// ---------- pair / sequence ----------

pub(super) fn cons(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(heap.alloc_pair(a, b))
}

/// Realise any lazy seq-view among `args`, returning a fresh vec with views
/// replaced by their realised lists (non-view args untouched). For the
/// stringifiers/printers, whose `&Heap` printer can't run a transducer. Fast path:
/// no view ⇒ a plain copy, no eval. Rooting: each `realize_seqview` can collect,
/// so every input and every already-realised result is kept on the root stack.
pub(super) fn realize_seqviews(
    heap: &mut Heap,
    env: EnvId,
    args: &[Value],
) -> Result<Vec<Value>, LispError> {
    if !args.iter().any(|a| matches!(a, Value::SeqView(_))) {
        return Ok(args.to_vec());
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
        Ok(out_roots.iter().map(|r| heap.read_root(*r)).collect())
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
        .map_get(m, Value::Keyword(crate::core::value::intern("__id__")))
        .is_none()
    {
        return Ok(None);
    }
    let genv = heap.global();
    let callee = heap
        .env_get(genv, crate::core::value::intern("->seq"))
        .ok_or_else(|| LispError::runtime("->seq: the Seqable protocol is unavailable"))?;
    Ok(Some(crate::eval::compile::apply_value(
        heap,
        callee,
        &[v],
        genv,
    )?))
}

pub(super) fn first(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v0 = arg(args, 0);
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
        Value::Set(id) => Ok(heap.set_elems(id).first().copied().unwrap_or(Value::nil())),
        // A map seqs as its `[k v]` pairs — the same view `seq`/`map`/`fold`/`last`
        // already take, so `first`/`rest` no longer erred on the one collection
        // every other seq op accepted.
        Value::Map(id) => match heap.map_first_entry(id) {
            Some((k, val)) => Ok(heap.alloc_vector(vec![k, val])),
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

pub(super) fn rest(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v0 = arg(args, 0);
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
            let entries: Vec<(Value, Value)> = heap.map_entries(id).into_iter().skip(1).collect();
            let pairs: Vec<Value> = entries
                .into_iter()
                .map(|(k, val)| heap.alloc_vector(vec![k, val]))
                .collect();
            Ok(heap.list(pairs))
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

pub(super) fn is_empty(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let x0 = arg(args, 0);
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
            Ok(heap.alloc_vector(vec![source, xform]))
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

/// `(%range-reduce f acc rng)` — left-fold a range with `f` in a native counted
/// loop, **without materialising** it: the whole point of the reducible range.
/// `acc` and `f` are rooted across the loop because each `apply` is a safepoint
/// that can relocate them.
pub(super) fn range_reduce(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let (lo, hi, step) = match arg(args, 2) {
        Value::Range(id) => heap.range_parts(id),
        Value::Nil => return Ok(init), // empty range — acc unchanged
        v => return Err(LispError::wrong_type(heap, "%range-reduce", "range", v)),
    };
    // Route the per-element callback through the VM when it's the active engine.
    // Hoisted out of the per-element loop deliberately: the choice is read once here and the
    // loop below branches on the resulting bool, so the ladder costs nothing per element.
    let use_vm = crate::eval::compile::tier_ceiling() >= crate::eval::compile::Tier::Bytecode;
    // Primitive-reducer fast path: when `f` is `+`/`*` (directly, or via the
    // prelude wrapper's passthrough arm), fold with the inlined scalar op and
    // never call back into `apply` per element.
    let prim = crate::eval::compile::reduce_prim_op(heap, f);

    // Tight i64 loop: when both prim resolves AND the accumulator is a plain i64,
    // operate on raw integers with no Value boxing per iteration. This avoids the
    // 24-byte-by-pointer passing overhead of `prim_apply_step` and the root
    // machinery (integers are inline — no GC slot needed). On overflow (rare),
    // fall through to the general path starting from the current position.
    if let (Some(op), Some(mut int_acc)) = (prim, init.as_int()) {
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            match crate::eval::compile::prim_apply_int_step(op, int_acc, i) {
                Some(v) => int_acc = v,
                None => {
                    // Overflow or unsupported op — hand off the remainder to the
                    // slow path starting from the current (i, acc) state.
                    return range_reduce_slow(
                        f,
                        Value::int(int_acc),
                        i,
                        hi,
                        step,
                        use_vm,
                        env,
                        heap,
                    );
                }
            }
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        return Ok(Value::int(int_acc));
    }

    range_reduce_slow(f, init, lo, hi, step, use_vm, env, heap)
}

/// `(%vector-reduce f acc v)` — left-fold a vector **by index** in a native loop.
///
/// The vector counterpart of [`range_reduce`], and it exists for the same reason: a
/// Brood-level fold pays a per-element `apply` that a native loop does not. The prelude's
/// `fold-vec` already dropped the `first`/`rest` list materialisation, but each element
/// still round-trips through the evaluator to call `f`, and — the part that costs the most
/// here — a reducer like `+` is a thin **passthrough wrapper**, so every element pays the
/// wrapper's redirect. [`reduce_prim_op`] resolves that wrapper ONCE (it is what makes
/// `(fold + 0 (range n))` fast today), but nothing on the vector path consulted it.
///
/// Measured on the `spawn-live` shape (100k fresh processes each folding a 16-cell payload,
/// the published row's exact `(fold + 0 p)`): the fold step cost **13.6 µs/unit** through
/// `fold-vec` against **8.8 µs** for the same fold written as `(fold %add 0 p)` — i.e. ~4.8 µs
/// of every unit was the passthrough redirect alone, on a row whose total is ~34 µs.
///
/// Ordering and semantics match `fold-vec` exactly: left-to-right, `(f acc item)`, and the
/// element is re-read from the (rooted) vector each step because every `apply` is a GC
/// safepoint that can relocate it.
pub(super) fn vector_reduce(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let vid = match arg(args, 2) {
        Value::Vector(id) => id,
        Value::Nil => return Ok(init),
        v => return Err(LispError::wrong_type(heap, "%vector-reduce", "vector", v)),
    };
    let n = heap.vector(vid).len();
    // Hoisted out of the per-element loop deliberately: the choice is read once here and the
    // loop below branches on the resulting bool, so the ladder costs nothing per element.
    let use_vm = crate::eval::compile::tier_ceiling() >= crate::eval::compile::Tier::Bytecode;
    // Primitive-reducer fast path: `+`/`*` directly, or through the prelude wrapper's
    // passthrough arm. This is the resolution the vector path never did.
    let prim = crate::eval::compile::reduce_prim_op(heap, f);

    // Tight i64 loop — no Value boxing, no root slot per element (integers are inline).
    // Mirrors `range_reduce`'s, including handing the remainder to the general loop on
    // overflow so a BigInt promotion stays bit-identical to the Brood fold.
    if let (Some(op), Some(mut int_acc)) = (prim, init.as_int()) {
        let mut i = 0usize;
        while i < n {
            let Some(x) = heap.vector(vid)[i].as_int() else {
                break; // non-int element — finish on the general path from here
            };
            match crate::eval::compile::prim_apply_int_step(op, int_acc, x) {
                Some(v) => int_acc = v,
                None => break, // overflow → general path from the current state
            }
            i += 1;
        }
        if i == n {
            return Ok(Value::int(int_acc));
        }
        return vector_reduce_general(f, Value::int(int_acc), vid, i, n, prim, use_vm, env, heap);
    }
    vector_reduce_general(f, init, vid, 0, n, prim, use_vm, env, heap)
}

/// The boxed/general half of [`vector_reduce`]: a non-int accumulator, a non-prim reducer,
/// or the tail of a fold that overflowed out of the i64 loop. Same three-tier step as
/// [`range_reduce_slow`] — inlined prim, else the resolved-once HOF arm, else a full apply.
#[allow(clippy::too_many_arguments)]
fn vector_reduce_general(
    f: Value,
    init: Value,
    vid: crate::core::value::VecId,
    start: usize,
    n: usize,
    prim: Option<crate::eval::compile::PrimOp>,
    use_vm: bool,
    env: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let hof = if prim.is_none() && use_vm {
        crate::eval::compile::hof_resolve(heap, f, 2)
    } else {
        None
    };
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let v_r = heap.root(Value::Vector(vid));
        let mut acc_r = heap.root(init);
        let mut i = start;
        while i < n {
            let f = heap.read_root(f_r);
            let acc = heap.read_root(acc_r);
            // Re-read the vector through its root: an `apply` below may have collected.
            let x = match heap.read_root(v_r) {
                Value::Vector(id) => heap.vector(id)[i],
                _ => break,
            };
            let step_call = |heap: &mut Heap, acc: Value| -> LispResult {
                if let Some(h) = &hof {
                    if let Some(r) = crate::eval::compile::hof_apply_step(heap, h, f, &[acc, x]) {
                        return r;
                    }
                }
                if use_vm {
                    crate::eval::compile::apply_value(heap, f, &[acc, x], env)
                } else {
                    apply(heap, f, &[acc, x], env)
                }
            };
            let next = match prim {
                Some(op) => match crate::eval::compile::prim_apply_step(op, acc, x)? {
                    Some(v) => v,
                    None => step_call(heap, acc)?,
                },
                None => step_call(heap, acc)?,
            };
            acc_r = heap.advance_root(acc_r, next);
            i += 1;
        }
        Ok(heap.read_root(acc_r))
    })
}

pub(super) fn range_reduce_slow(
    f: Value,
    init: Value,
    lo: i64,
    hi: i64,
    step: i64,
    use_vm: bool,
    env: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let prim = crate::eval::compile::reduce_prim_op(heap, f);
    // HOF fast path (gated): resolve the step closure's arm ONCE so the per-element call skips
    // arm re-resolution + passthrough/arity matching. Only for a non-prim reducer on the VM path.
    let hof = if prim.is_none() && use_vm {
        crate::eval::compile::hof_resolve(heap, f, 2)
    } else {
        None
    };
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let mut acc_r = heap.root(init);
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            let f = heap.read_root(f_r);
            let acc = heap.read_root(acc_r);
            // Non-prim step: try the cached-arm fast path (falls back if `f` late-rebound or
            // the gate is off, i.e. `hof` is `None`).
            let step_call = |heap: &mut Heap, acc: Value| -> LispResult {
                if let Some(h) = &hof {
                    if let Some(r) =
                        crate::eval::compile::hof_apply_step(heap, h, f, &[acc, Value::int(i)])
                    {
                        return r;
                    }
                }
                if use_vm {
                    crate::eval::compile::apply_value(heap, f, &[acc, Value::int(i)], env)
                } else {
                    apply(heap, f, &[acc, Value::int(i)], env)
                }
            };
            let next = match prim {
                Some(op) => match crate::eval::compile::prim_apply_step(op, acc, Value::int(i))? {
                    Some(v) => v,
                    None => step_call(heap, acc)?,
                },
                None => step_call(heap, acc)?,
            };
            acc_r = heap.advance_root(acc_r, next);
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        Ok(heap.read_root(acc_r))
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
    // Measured A/B on one binary, benchmark suite's `sort` row (375k ints):
    // the sort call itself 106 -> 79 ms, the whole row 225 -> 196 ms. Note
    // what that does NOT say: comparison was only ~27 ms of the original
    // 106 ms. The rest is `seq_items` walking the cons spine in and
    // `heap.list` allocating a fresh 375k-cell list out, and this fast path
    // does not touch either. Anyone chasing the remaining ~79 ms should go
    // after the traversal/rebuild (i.e. allocation), not the comparator —
    // sorting is no longer the expensive part of `sort`.
    //
    // `sort_unstable` is safe to use here even though the general path is
    // stable: two equal `Int`s are the same value, so no observable ordering
    // distinguishes them. That does NOT hold for the mixed path, where an
    // Int and a Float can compare equal while remaining distinguishable
    // (`1` vs `1.0`), which is why only this branch drops stability.
    if all_int {
        ints.sort_unstable();
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
    items.sort_by(|a, b| heap.value_cmp(*a, *b));

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
    items.sort_by(|a, b| heap.value_cmp(*a, *b));
    Ok(heap.list(items))
}

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

// ---------- vector ----------

pub(super) fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector(args.to_vec()))
}

pub(super) fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let n = expect_int(heap, "vector-ref", arg(args, 1))?;
    match v {
        Value::Vector(id) if n >= 0 && (n as usize) < heap.vector(id).len() => {
            Ok(heap.vector(id)[n as usize])
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "vector-ref: index {} out of range [0, {})",
            n,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "vector-ref", "vector", v)),
    }
}

pub(super) fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Vector(id) => Ok(Value::int(heap.vector(id).len() as i64)),
        _ => Err(LispError::wrong_type(heap, "vector-length", "vector", v)),
    }
}

/// `(vector-assoc v i x)` — a fresh vector like `v` with index `i` set to `x`.
/// The vector counterpart of `%map-assoc`; O(n) copy (vectors are flat), one
/// allocation, no cons churn. `i` must be in `[0, len)` (append-at-end is a
/// deferred power feature, ADR-011). No GC safepoint runs inside a builtin, so
/// the cloned handles stay valid across `alloc_vector`.
pub(super) fn vector_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let i = expect_int(heap, "vector-assoc", arg(args, 1))?;
    let x = arg(args, 2);
    match v {
        Value::Vector(id) if i >= 0 && (i as usize) < heap.vector(id).len() => {
            let mut items = heap.vector(id).to_vec();
            items[i as usize] = x;
            Ok(heap.alloc_vector(items))
        }
        Value::Vector(id) => Err(LispError::runtime(format!(
            "vector-assoc: index {} out of range [0, {})",
            i,
            heap.vector(id).len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)),
        _ => Err(LispError::wrong_type(heap, "vector-assoc", "vector", v)),
    }
}

/// `(subvec v start)` / `(subvec v start end)` — a fresh vector of the elements
/// of `v` in `[start, end)` (`end` defaults to the length). `0 <= start <= end
/// <= len`; out of range is an error. The slice counterpart of `substring`, and
/// the vector-preserving slice the list-returning `take`/`drop` don't give.
pub(super) fn subvec(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let id = match v {
        Value::Vector(id) => id,
        _ => return Err(LispError::wrong_type(heap, "subvec", "vector", v)),
    };
    let len = heap.vector(id).len() as i64;
    let start = expect_int(heap, "subvec", arg(args, 1))?;
    let end = if args.len() > 2 {
        expect_int(heap, "subvec", arg(args, 2))?
    } else {
        len
    };
    if start < 0 || end > len || start > end {
        return Err(LispError::runtime(format!(
            "subvec: range [{start}, {end}) out of bounds for vector of length {len}"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let items = heap.vector(id)[start as usize..end as usize].to_vec();
    Ok(heap.alloc_vector(items))
}

// ---------- map ----------

/// Require a map; otherwise a self-identifying type error attributed to `who`.
pub(super) fn expect_map(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    expect!(heap, who, v, "map",
        Value::Map(id) => id,
    )
}

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

/// The `[k v]` of a pair item — a `[k v]` vector or a `(k v)` list — with
/// `first`/`second` semantics (missing slots read as `nil`). Used by
/// [`map_into`] to read the items of an `into`/`zipmap` sequence.
pub(super) fn pair_kv(heap: &Heap, who: &str, p: Value) -> Result<(Value, Value), LispError> {
    match p {
        Value::Vector(id) => {
            let v = heap.vector(id);
            Ok((
                v.first().copied().unwrap_or(Value::nil()),
                v.get(1).copied().unwrap_or(Value::nil()),
            ))
        }
        Value::Pair(id) => {
            let (k, rest) = heap.pair(id);
            let val = match rest {
                Value::Pair(rid) => heap.pair(rid).0,
                _ => Value::nil(),
            };
            Ok((k, val))
        }
        _ => Err(LispError::wrong_type(heap, who, "pair or vector", p)),
    }
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

/// `(%registry-update! name op path value)` — atomically read-modify-write a global that
/// holds a registry (KI-22). `name` is the global's symbol, `op` one of `:assoc`,
/// `:assoc-new`, `:dissoc`, `:cons-new`, `path` a vector of one or two keys (nil for
/// `:cons-new`). Returns true if the registry was written, false if the op declined.
///
/// The whole sequence runs inside [`Heap::registry_update`] under the runtime's registry
/// lock — see there for why `(def *X* (assoc *X* …))` in Brood cannot be made safe.
pub(super) fn registry_update(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    use crate::core::heap::RegistryOp;
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-update!",
                "symbol",
                v,
            ))
        }
    };
    // Keywords are interned, so compare the interned ids rather than the Values.
    let op_sym = match arg(args, 1) {
        Value::Keyword(k) => k,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-update!",
                "keyword",
                v,
            ))
        }
    };
    let op = if op_sym == value::intern("assoc") {
        RegistryOp::Assoc
    } else if op_sym == value::intern("assoc-new") {
        RegistryOp::AssocNew
    } else if op_sym == value::intern("dissoc") {
        RegistryOp::Dissoc
    } else if op_sym == value::intern("cons-new") {
        RegistryOp::ConsNew
    } else {
        return Err(LispError::type_err(
            "%registry-update!: op must be :assoc, :assoc-new, :dissoc or :cons-new",
        ));
    };
    let path = match arg(args, 2).unpack() {
        crate::core::value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => Vec::new(),
    };
    Ok(Value::boolean(heap.registry_update(
        env,
        sym,
        op,
        &path,
        arg(args, 3),
    )))
}

/// `(%registry-member? name key)` — is registry global `name` a map containing `key`, read
/// from the shared globals table bypassing the per-process inline cache (ADR-225)? For a
/// load-once guard that must not miss a racing `provide`; see [`Heap::registry_member`].
pub(super) fn registry_member(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%registry-member?",
                "symbol",
                v,
            ))
        }
    };
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
    let sym = match arg(args, 0) {
        Value::Sym(s) => s,
        v => return Err(LispError::wrong_type(heap, "%registry-cas!", "symbol", v)),
    };
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

/// `(%map-int-add m k delta)` — a fresh map with `k`'s integer value incremented
/// by `delta` (inserts `delta` when `k` is absent). Single trie traversal. Raises
/// past the i64 range, like `table-incr` — which the linear-map optimizer rewrites
/// this into, so the two agree (see [`Heap::map_int_add`]).
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

/// `(%map-pairs m)` — the entries as a list of `[k v]` vectors, in insertion
/// order, in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`reduce-kv` are all Brood over it (std/prelude.blsp).
pub(super) fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-pairs", arg(args, 0))?;
    let entries = heap.map_entries(id); // copy out, releasing the borrow before we alloc
    let pairs: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| heap.alloc_vector(vec![k, v]))
        .collect();
    Ok(heap.list(pairs))
}

/// `(%map-count m)` — the number of entries, O(1). The CHAMP root node tracks
/// its subtree size, so this never walks (or allocates) the entries; it's what
/// `count`/`empty?` on a map use instead of materialising `%map-pairs`.
pub(super) fn map_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "%map-count", arg(args, 0))?;
    Ok(Value::int(heap.map_size(id) as i64))
}

pub(super) fn expect_set(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    expect!(heap, who, v, "set",
        Value::Set(id) => id,
    )
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
    Ok(heap.set_from_elems(args.to_vec()))
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
        // O(1): the char count is cached on the slot. It used to be `chars().count()`
        // here — a full scan on every call, which is why a loop bounded by
        // `(string/length s)` was quadratic before it did anything else.
        Value::Str(id) => Ok(Value::int(heap.str_metrics(id).0 as i64)),
        _ => Err(LispError::wrong_type(heap, "string/length", "string", v)),
    }
}

/// `(string/display-width s)` — how many terminal/grid *cells* `s` occupies, counting
/// grapheme clusters (an emoji / flag / CJK char is 2, a combining mark 0). The
/// width-aware counterpart to `string-length` (which counts codepoints) — the
/// editor's column / cursor math uses it so a wide glyph advances two columns. The
/// GUI renderer advances the cell grid by the same measure (`crate::host::text_width`).
pub(super) fn display_width(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::int(
            crate::host::text_width::display_width(&heap.string(id)) as i64,
        )),
        _ => Err(LispError::wrong_type(
            heap,
            "string/display-width",
            "string",
            v,
        )),
    }
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
