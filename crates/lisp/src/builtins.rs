//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in Brood lives in `std/prelude.blsp` instead.
//! `%`-prefixed names are low-level primitives not meant to be called directly.
//! The annotated list is in `docs/primitives.md`.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Arity, EnvId, NativeFn, NativeFnPtr, Tag, Value};
use crate::error::{LispError, LispResult};
use crate::eval::{apply, compile::apply_engine};
use crate::syntax::{cst, printer, reader};
use crate::types::{Sig, Ty};

/// Install the primitive kernel into `root`.
#[allow(non_upper_case_globals)] // the `const` type shorthands below read as locals
pub fn register(heap: &mut Heap, root: EnvId) {
    let def = |heap: &mut Heap, name: &str, arity: Arity, sig: Sig, func: NativeFnPtr| {
        let (params, doc) = primitive_doc(name);
        let v = heap.alloc_native(NativeFn {
            name: name.to_string(),
            arity,
            sig,
            func,
            params,
            doc,
        });
        heap.env_define(root, value::intern(name), v);
    };

    // Lattice shorthands used in the signatures below; see types::Ty for the
    // algebra. NUMBER = int ∪ float, LIST = nil ∪ pair, seq = list ∪ vector
    // (the receivers of first/rest). `callable` = fn ∪ native (a thunk or
    // applicable). `ANY` is the "no useful info" lane — overlaps everything,
    // so the disjointness checker never warns against it.
    // `const` (not `let`) so each of the 170-odd uses below re-materialises a
    // fresh `Ty` — `Ty` is no longer `Copy` (it carries an optional `Arc` arrow
    // refinement, ADR-078), but a `const` mention is inlined, so reusing these
    // shorthands by value needs no `.clone()`. Lowercase names kept (they read as
    // type shorthands, not globals); hence the `allow` on the enclosing fn.
    const any: Ty = Ty::ANY;
    const int: Ty = Ty::of(Tag::Int);
    const num: Ty = Ty::NUMBER;
    const float: Ty = Ty::of(Tag::Float);
    const string: Ty = Ty::of(Tag::Str);
    const rope: Ty = Ty::of(Tag::Rope);
    const socket_ty: Ty = Ty::of(Tag::Socket);
    const subprocess_ty: Ty = Ty::of(Tag::Subprocess);
    const table_ty: Ty = Ty::of(Tag::Table);
    const kw: Ty = Ty::of(Tag::Keyword);
    const sym: Ty = Ty::of(Tag::Sym);
    const bool_ty: Ty = Ty::of(Tag::Bool);
    const nil_ty: Ty = Ty::of(Tag::Nil);
    const pair: Ty = Ty::of(Tag::Pair);
    const vec_ty: Ty = Ty::of(Tag::Vector);
    const map_ty: Ty = Ty::of(Tag::Map);
    const transient_ty: Ty = Ty::of(Tag::Transient);
    const pid_ty: Ty = Ty::of(Tag::Pid);
    const ref_ty: Ty = Ty::of(Tag::Ref);
    const list_ty: Ty = Ty::LIST;
    const seq: Ty = Ty::of_tags(&[Tag::Nil, Tag::Pair, Tag::Vector]);
    const callable: Ty = Ty::of_tags(&[Tag::Fn, Tag::Native]);

    // numeric primitives — `%add`..`%div` accept and return the wider NUMBER
    // (int + int may overflow into Float; the others always do on a Float arg).
    // `%lt` is comparison → bool; `%eq` accepts anything and returns bool.
    def(
        heap,
        "%add",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_add,
    );
    def(
        heap,
        "%sub",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_sub,
    );
    def(
        heap,
        "%mul",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_mul,
    );
    def(
        heap,
        "%div",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        prim_div,
    );
    def(
        heap,
        "%lt",
        Arity::exact(2),
        Sig::new(vec![num, num], bool_ty),
        prim_lt,
    );
    def(
        heap,
        "%le",
        Arity::exact(2),
        Sig::new(vec![num, num], bool_ty),
        prim_le,
    );
    def(
        heap,
        kw::EQ_PRIM,
        Arity::exact(2),
        Sig::new(vec![any, any], bool_ty),
        prim_eq,
    );
    // `mod` is Brood over `rem` (std/prelude.blsp); only `rem` is primitive.
    def(
        heap,
        "rem",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        remainder,
    );
    // `%quot` — truncating integer division (toward zero), the kernel `quot`
    // passes through to so the VM inlines it as one op. (It used to be Brood over
    // `(/ (- a (rem a b)) b)` — three dispatched calls per use, which made tight
    // integer loops like `collatz` pay rem+sub+div every step.)
    def(
        heap,
        "%quot",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        prim_quot,
    );
    // `floor` is the single irreducible Float→Int crossing; ceil/round/pow/
    // sqrt are all Brood over it + rem/`/`/`*`/`<` (std/prelude.blsp).
    def(
        heap,
        "floor",
        Arity::exact(1),
        Sig::new(vec![num], int),
        floor,
    );

    // bitwise — integer bit-twiddling on the i64 two's-complement representation.
    // Table stakes for hashing, flags, and PRNGs (the std xorshift PRNG is built
    // on these); they were a noted gap (docs/feedback-retro-game-of-life.md).
    def(
        heap,
        "bit-and",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_and,
    );
    def(
        heap,
        "bit-or",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_or,
    );
    def(
        heap,
        "bit-xor",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_xor,
    );
    def(
        heap,
        "bit-not",
        Arity::exact(1),
        Sig::new(vec![int], int),
        bit_not,
    );
    def(
        heap,
        "bit-shift-left",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_shift_left,
    );
    def(
        heap,
        "bit-shift-right",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        bit_shift_right,
    );
    def(
        heap,
        "bit-count",
        Arity::exact(1),
        Sig::new(vec![int], int),
        bit_count,
    );
    def(
        heap,
        "bit-positions",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        bit_positions,
    );
    // --- bitset: a fixed-size bit array backed by a refc SharedBlob (POC, ADR-107-adjacent).
    // A `bitset` is a `Str` whose bytes are raw bit-data (LSB-first, bit i = byte i/8 bit i%8),
    // ALWAYS stored shared, so it crosses `send`/`table` by reference (Arc bump), not a copy —
    // unlike a bignum, which serialises to decimal. Bitwise ops are O(bytes) native loops.
    def(heap, "bitset", Arity::exact(1), Sig::new(vec![int], string), bs_make);
    def(heap, "bitset-ones", Arity::exact(1), Sig::new(vec![int], string), bs_ones);
    def(heap, "bitset-and", Arity::exact(2), Sig::new(vec![string, string], string), bs_and);
    def(heap, "bitset-or", Arity::exact(2), Sig::new(vec![string, string], string), bs_or);
    def(heap, "bitset-xor", Arity::exact(2), Sig::new(vec![string, string], string), bs_xor);
    def(heap, "bitset-shl", Arity::exact(2), Sig::new(vec![string, int], string), bs_shl);
    def(heap, "bitset-shr", Arity::exact(2), Sig::new(vec![string, int], string), bs_shr);
    def(heap, "bitset-set", Arity::exact(2), Sig::new(vec![string, int], string), bs_set);
    def(heap, "bitset-count", Arity::exact(1), Sig::new(vec![string], int), bs_count);
    def(heap, "bitset-positions", Arity::exact(1), Sig::new(vec![string], vec_ty), bs_positions);
    def(heap, "bitset-planes", Arity::exact(1), Sig::new(vec![vec_ty], vec_ty), bs_planes);
    def(
        heap,
        "bitset-neighbour-sum",
        Arity::exact(7),
        Sig::new(vec![string, string, string, string, string, int, int], vec_ty),
        bs_neighbour_sum,
    );
    def(
        heap,
        "bitset-life-step",
        Arity::exact(7),
        Sig::new(vec![string, string, string, string, string, int, int], string),
        bs_life_step,
    );

    // pair / sequence — `empty?` is Brood (type dispatch over string-length /
    // vector-length / map-keys; std/prelude.blsp). `first`/`rest` ARE the pair
    // accessors (car/cdr), so they stay. `rest` always yields a list (a vector's
    // tail is built via `heap.list`), never a vector.
    def(
        heap,
        "cons",
        Arity::exact(2),
        Sig::new(vec![any, any], pair),
        cons,
    );
    def(
        heap,
        "first",
        Arity::exact(1),
        Sig::new(vec![seq], any),
        first,
    );
    def(
        heap,
        "rest",
        Arity::exact(1),
        Sig::new(vec![seq], list_ty),
        rest,
    );
    // Lazy reducible range (ADR: reducible range). `%range` constructs it (arg
    // parsing is in the Brood `range`); the fold-family fast paths in the prelude
    // call `range?` / `%range-reduce` / `%range-count`; everything else realises
    // via `%range->list`. A range carries `tag = Pair`, so its surface type is a
    // list — hence the `list_ty` sigs.
    def(
        heap,
        "%range",
        Arity::exact(3),
        Sig::new(vec![int, int, int], list_ty),
        range_make,
    );
    def(
        heap,
        "range?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        range_pred,
    );
    def(
        heap,
        "%range-count",
        Arity::exact(1),
        Sig::new(vec![list_ty], int),
        range_count,
    );
    def(
        heap,
        "%range->list",
        Arity::exact(1),
        Sig::new(vec![list_ty], list_ty),
        range_to_list,
    );
    def(
        heap,
        "%range-reduce",
        Arity::exact(3),
        Sig::new(vec![callable, any, list_ty], any),
        range_reduce,
    );
    // `%sort-asc` is the Rust fast path for the common `(sort coll)` case
    // (ascending by `<`, no custom comparator). Avoids per-comparison Brood
    // eval overhead — the old in-Brood mergesort was ~1.5 s on 10 000 items
    // because every compare went through `eval::apply`. `sort-by` /
    // `(sort cmp coll)` still routes through the Brood merge sort for
    // arbitrary comparators. Items must be all-`int` or all-`float`; mixed
    // numerics work by promotion (matches `<`'s semantics).
    def(
        heap,
        "%sort-asc",
        Arity::exact(1),
        Sig::new(vec![seq], list_ty),
        sort_asc,
    );
    // `%sort-cmp` is the non-numeric fallback for `(sort coll)`: sorts via the
    // Rust-side structural total order (`value_cmp`). Lets `(sort [[1 0] [2 1]])`
    // and the like work without a custom comparator. Brood `sort` (prelude)
    // dispatches: numeric items go through `%sort-asc` (faster), anything else
    // through `%sort-cmp`.
    def(
        heap,
        "%sort-cmp",
        Arity::exact(1),
        Sig::new(vec![seq], list_ty),
        sort_cmp,
    );
    // `(compare a b)` exposes the same structural total order as a binary
    // comparison (-1/0/1), so `sort-by` / `min-by` / custom comparators work over
    // any orderable value (strings, keywords, vectors, …), not just numbers.
    def(
        heap,
        "compare",
        Arity::exact(2),
        Sig::new(vec![any, any], int),
        compare,
    );

    // vector
    def(
        heap,
        "vector",
        Arity::any(),
        Sig::variadic(any, vec_ty),
        vector,
    );
    def(
        heap,
        "vector-ref",
        Arity::exact(2),
        Sig::new(vec![vec_ty, int], any),
        vector_ref,
    );
    def(
        heap,
        "vector-length",
        Arity::exact(1),
        Sig::new(vec![vec_ty], int),
        vector_length,
    );
    def(
        heap,
        "vector-assoc",
        Arity::exact(3),
        Sig::new(vec![vec_ty, int, any], vec_ty),
        vector_assoc,
    );
    def(
        heap,
        "subvec",
        Arity::range(2, 3),
        Sig::with_rest(vec![vec_ty, int], int, vec_ty),
        subvec,
    );

    // map — the *minimal* kernel: construct, read, two producers, and one
    // enumerator (`map-pairs` → [k v] vectors). `keys`/`vals`/`contains?`/
    // `reduce-kv` and the `get`/`assoc`/`dissoc` surface (variadic + defaults) are
    // all Brood over these (std/prelude.blsp). Maps are immutable: each op returns
    // a fresh map.
    def(
        heap,
        "hash-map",
        Arity::any(),
        Sig::variadic(any, map_ty),
        hash_map,
    );
    def(
        heap,
        "map-get",
        Arity::range(2, 3),
        Sig::with_rest(vec![map_ty, any], any, any),
        map_get,
    );
    def(
        heap,
        "map-assoc",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, any], map_ty),
        map_assoc,
    );
    def(
        heap,
        "map-dissoc",
        Arity::exact(2),
        Sig::new(vec![map_ty, any], map_ty),
        map_dissoc,
    );
    def(
        heap,
        "map-pairs",
        Arity::exact(1),
        Sig::new(vec![map_ty], list_ty),
        map_pairs,
    );
    def(
        heap,
        "map-count",
        Arity::exact(1),
        Sig::new(vec![map_ty], int),
        map_count,
    );
    def(
        heap,
        "%map-into",
        Arity::exact(2),
        Sig::new(vec![map_ty, any], map_ty),
        map_into,
    );

    // transient maps (Clojure's transient/assoc!/dissoc!/persistent!)
    def(
        heap,
        "transient",
        Arity::exact(1),
        Sig::new(vec![map_ty], transient_ty),
        transient,
    );
    def(
        heap,
        "assoc!",
        Arity::exact(3),
        Sig::new(vec![transient_ty, any, any], transient_ty),
        transient_assoc,
    );
    def(
        heap,
        "dissoc!",
        Arity::exact(2),
        Sig::new(vec![transient_ty, any], transient_ty),
        transient_dissoc,
    );
    def(
        heap,
        "persistent!",
        Arity::exact(1),
        Sig::new(vec![transient_ty], map_ty),
        transient_persistent,
    );
    def(
        heap,
        "transient?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        transient_pred,
    );
    // `transient-get` / `transient-count` / `transient-contains?` — the kernel
    // hooks the prelude's `get`/`count`/`contains?` dispatch to when handed a
    // transient (a live transient supports lookups, Clojure-style).
    def(
        heap,
        "transient-get",
        Arity::range(2, 3),
        Sig::with_rest(vec![transient_ty, any], any, any),
        transient_get,
    );
    def(
        heap,
        "transient-count",
        Arity::exact(1),
        Sig::new(vec![transient_ty], int),
        transient_count,
    );
    def(
        heap,
        "transient-contains?",
        Arity::exact(2),
        Sig::new(vec![transient_ty, any], bool_ty),
        transient_contains,
    );

    // string
    def(
        heap,
        "string-length",
        Arity::exact(1),
        Sig::new(vec![string], int),
        string_length,
    );
    def(
        heap,
        "substring",
        Arity::range(2, 3),
        Sig::with_rest(vec![string, int], int, string),
        substring,
    );
    def(
        heap,
        "string-span",
        Arity::exact(3),
        Sig::new(vec![string, int, string], int),
        string_span,
    );
    def(
        heap,
        "string-span-until",
        Arity::exact(3),
        Sig::new(vec![string, int, string], int),
        string_span_until,
    );
    def(
        heap,
        "display-width",
        Arity::exact(1),
        Sig::new(vec![string], int),
        display_width,
    );
    // Linear substring search — like `substring`/`lower`, it genuinely needs Rust:
    // Brood has no O(1) char access (char indexing into UTF-8 is O(index)), so a
    // pure-Brood scan re-skips and is unavoidably O(n²) — which made `doc-search`'s
    // whole-namespace scan tens of seconds. `index-of` / `string-contains?` /
    // `includes?` (std/prelude.blsp) ride on this; it's the search counterpart of
    // the `substring` slice primitive.
    def(
        heap,
        "%str-index-of",
        Arity::exact(2),
        Sig::new(vec![string, string], int),
        str_index_of,
    );
    // Splitting genuinely needs Rust for the same reason as the search above: a
    // pure-Brood split re-`substring`s the tail each step, and char-indexed substring
    // is O(index), so the whole split is O(n²) — a 174 KB `git ls-files` output took
    // ~840 ms in the editor's project-file scan. Rust's `str::split` is one O(n) pass.
    def(
        heap,
        "string-split",
        Arity::exact(2),
        Sig::new(vec![string, string], list_ty),
        string_split,
    );
    // Case folding (Unicode tables) and parse-or-nil genuinely need Rust; the rest
    // of the string library (split/join/replace/index-of/trim/…) is Brood over
    // these + `substring`/`%str-index-of`/`str` (std/prelude.blsp).
    def(
        heap,
        "upper",
        Arity::exact(1),
        Sig::new(vec![string], string),
        upper,
    );
    def(
        heap,
        "lower",
        Arity::exact(1),
        Sig::new(vec![string], string),
        lower,
    );
    // Codepoint ↔ char and byte-level UTF-8 access — the primitives encoding
    // modules need that can't be written in Brood over `substring` alone.
    def(
        heap,
        "char->int",
        Arity::exact(1),
        Sig::new(vec![string], int),
        char_to_int,
    );
    def(
        heap,
        "int->char",
        Arity::exact(1),
        Sig::new(vec![int], string),
        int_to_char,
    );
    def(
        heap,
        "string->utf8-bytes",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        string_to_utf8_bytes,
    );
    def(
        heap,
        "utf8-bytes->string",
        Arity::exact(1),
        Sig::new(vec![vec_ty], string),
        utf8_bytes_to_string,
    );
    // string->number returns int *or* float *or* nil (the parse-failed case).
    def(
        heap,
        "string->number",
        Arity::exact(1),
        Sig::new(vec![string], num.union(nil_ty)),
        string_to_number,
    );
    // `to-fixed` renders a number with a fixed count of decimals — the one
    // float→text op `str`/`pr-str` can't express (they print shortest round-trip
    // form, i.e. full f64 precision). `round-to` (a *number*) is Brood over floor.
    def(
        heap,
        "to-fixed",
        Arity::exact(2),
        Sig::new(vec![num, int], string),
        to_fixed,
    );

    // transcendental math — hardware f64 ops that can't be approximated in Brood
    // over `floor`/`rem`/`*` at the precision level scripts actually need.
    def(
        heap,
        "sin",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_sin,
    );
    def(
        heap,
        "cos",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_cos,
    );
    def(
        heap,
        "tan",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_tan,
    );
    def(
        heap,
        "asin",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_asin,
    );
    def(
        heap,
        "acos",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_acos,
    );
    def(
        heap,
        "atan",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_atan,
    );
    def(
        heap,
        "atan2",
        Arity::exact(2),
        Sig::new(vec![num, num], float),
        math_atan2,
    );
    def(
        heap,
        "exp",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_exp,
    );
    def(
        heap,
        "ln",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_ln,
    );
    def(
        heap,
        "log2",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_log2,
    );
    def(
        heap,
        "log10",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_log10,
    );

    // rope — the editor buffer's text storage (ADR-045). The irreducible text
    // mechanism: a `ropey::Rope` gives O(log n) edits + char/line indexing that
    // Brood can't bootstrap over flat strings. Immutable like every value —
    // `rope-insert`/`rope-delete` return a *fresh* rope (cheap structural share).
    // Points, marks, regions, search, the buffer process itself: all Brood above.
    def(
        heap,
        "string->rope",
        Arity::exact(1),
        Sig::new(vec![string], rope),
        string_to_rope,
    );
    def(
        heap,
        "rope->string",
        Arity::exact(1),
        Sig::new(vec![rope], string),
        rope_to_string,
    );
    def(
        heap,
        "rope-length",
        Arity::exact(1),
        Sig::new(vec![rope], int),
        rope_length,
    );
    def(
        heap,
        "rope-line-count",
        Arity::exact(1),
        Sig::new(vec![rope], int),
        rope_line_count,
    );
    def(
        heap,
        "rope-insert",
        Arity::exact(3),
        Sig::new(vec![rope, int, string], rope),
        rope_insert,
    );
    def(
        heap,
        "rope-delete",
        Arity::exact(3),
        Sig::new(vec![rope, int, int], rope),
        rope_delete,
    );
    def(
        heap,
        "rope-slice",
        Arity::exact(3),
        Sig::new(vec![rope, int, int], string),
        rope_slice,
    );
    def(
        heap,
        "rope-line",
        Arity::exact(2),
        Sig::new(vec![rope, int], string),
        rope_line,
    );
    def(
        heap,
        "rope-char->line",
        Arity::exact(2),
        Sig::new(vec![rope, int], int),
        rope_char_to_line,
    );
    def(
        heap,
        "rope-line->char",
        Arity::exact(2),
        Sig::new(vec![rope, int], int),
        rope_line_to_char,
    );

    // TCP sockets (ADR-062), built on the blocking-IO → mailbox seam (ADR-059):
    // inbound data is delivered to the owning process's mailbox as `[:tcp sock
    // data]` / `[:tcp-closed sock]` / `[:tcp-accept lsock client]` messages, which
    // Brood `receive`s — no polling, no worker ever blocked. `connect`/`listen`
    // register the *calling* process as the owner. A socket is an opaque handle,
    // valid across this runtime's processes, never sent across nodes.
    def(
        heap,
        "tcp-connect",
        Arity::exact(2),
        Sig::new(vec![string, int], socket_ty),
        tcp_connect,
    );
    def(
        heap,
        "tcp-listen",
        Arity::exact(2),
        Sig::new(vec![string, int], socket_ty),
        tcp_listen,
    );
    def(
        heap,
        "tls-request",
        Arity::exact(3),
        Sig::new(vec![string, int, string], socket_ty),
        tls_request,
    );
    def(
        heap,
        "tcp-send",
        Arity::exact(2),
        Sig::new(vec![socket_ty, string], nil_ty),
        tcp_send,
    );
    def(
        heap,
        "tcp-set-binary",
        Arity::exact(2),
        Sig::new(vec![socket_ty, bool_ty], nil_ty),
        tcp_set_binary,
    );
    def(
        heap,
        "tcp-controlling-process",
        Arity::exact(2),
        Sig::new(vec![socket_ty, pid_ty], nil_ty),
        tcp_controlling_process,
    );
    def(
        heap,
        "tcp-close",
        Arity::exact(1),
        Sig::new(vec![socket_ty], nil_ty),
        tcp_close,
    );
    // Persistent child processes (ADR-104): spawn a co-process with piped stdio,
    // write its stdin, receive its stdout/stderr as `[:proc …]` mailbox messages.
    // A `Value::Subprocess` handle, local to this runtime, never sent across nodes.
    def(
        heap,
        "proc-spawn",
        Arity::range(2, 3),
        Sig::with_rest(vec![string, list_ty.union(vec_ty)], map_ty, subprocess_ty),
        proc_spawn,
    );
    def(
        heap,
        "proc-send",
        Arity::exact(2),
        Sig::new(vec![subprocess_ty, string], nil_ty),
        proc_send,
    );
    def(
        heap,
        "proc-close",
        Arity::exact(1),
        Sig::new(vec![subprocess_ty], nil_ty),
        proc_close,
    );
    // In-memory shared table — Brood's ETS (ADR-107). A `Value::Table` handle into a
    // global registry of stores holding deep clones (Message form); sendable across
    // processes (every copy shares one store) but local to this runtime.
    def(heap, "table", Arity::exact(0), Sig::nullary(table_ty), table_new);
    def(
        heap,
        "table-put",
        Arity::exact(3),
        Sig::new(vec![table_ty, any, any], table_ty),
        table_put,
    );
    def(
        heap,
        "table-get",
        Arity::range(2, 3),
        Sig::new(vec![table_ty, any], any),
        table_get,
    );
    def(
        heap,
        "table-has?",
        Arity::exact(2),
        Sig::new(vec![table_ty, any], bool_ty),
        table_has,
    );
    def(
        heap,
        "table-delete",
        Arity::exact(2),
        Sig::new(vec![table_ty, any], table_ty),
        table_delete,
    );
    def(
        heap,
        "table-incr",
        Arity::range(2, 3),
        Sig::new(vec![table_ty, any], int),
        table_incr,
    );
    def(
        heap,
        "table-count",
        Arity::exact(1),
        Sig::new(vec![table_ty], int),
        table_count,
    );
    def(
        heap,
        "table-snapshot",
        Arity::exact(1),
        Sig::new(vec![table_ty], map_ty),
        table_snapshot,
    );
    def(
        heap,
        "table-drop",
        Arity::exact(1),
        Sig::new(vec![table_ty], bool_ty),
        table_drop,
    );
    def(
        heap,
        "tcp-local-port",
        Arity::exact(1),
        Sig::new(vec![socket_ty], int.union(nil_ty)),
        tcp_local_port,
    );

    // terminal frontend (ADR-046) — the thin crossterm seam that paints the
    // display protocol and reads keys. The protocol itself is Brood data (a
    // vector of render ops); these primitives are mechanism only. `term-poll`
    // returns a key (a 1-char string, or a keyword for specials) or nil on
    // timeout; `term-draw` interprets a frame vector. See std/observer.blsp.
    def(
        heap,
        "term-enter",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_enter,
    );
    def(
        heap,
        "term-leave",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_leave,
    );
    def(
        heap,
        "term-size",
        Arity::exact(0),
        Sig::new(vec![], vec_ty),
        term_size,
    );
    def(
        heap,
        "term-poll",
        Arity::exact(1),
        Sig::new(vec![int], string.union(kw).union(nil_ty)),
        term_poll,
    );
    def(
        heap,
        "term-draw",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        term_draw,
    );
    // Inline (relative-motion) variant of the seam, for an in-place line editor
    // that must NOT take over the screen: `term-raw-enter`/`term-raw-leave` toggle
    // raw mode only (no alternate screen, cursor stays visible, scrollback kept),
    // and `term-emit` paints relative ops. The self-hosted REPL editor uses these
    // (std/lineedit.blsp); `term-enter`/`term-draw` stay the full-screen path.
    def(
        heap,
        "term-raw-enter",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_raw_enter,
    );
    def(
        heap,
        "term-raw-leave",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        term_raw_leave,
    );
    def(
        heap,
        "term-emit",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        term_emit,
    );
    // The windowed (GUI) frontend — the same seam as `term-*`, painting the same
    // render-op protocol to a native window (feature "gui"; the symbols always
    // exist, erroring at call time without the feature). Unlike the single
    // terminal, there can be many windows: `gui-open` returns an integer window id
    // and the other primitives take it, so `(observe)` can spawn several at once.
    // std/observer.blsp's `gui-display` wraps an id as a display map. See gui.rs.
    def(
        heap,
        "gui-open",
        Arity::range(0, 3),
        Sig::new(vec![string, int, int], int),
        gui_open,
    );
    def(
        heap,
        "gui-close",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        gui_close,
    );
    def(
        heap,
        "gui-title!",
        Arity::exact(2),
        Sig::new(vec![int, string], nil_ty),
        gui_title,
    );
    def(
        heap,
        "gui-icon!",
        Arity::exact(4),
        Sig::new(vec![int, vec_ty, int, int], nil_ty),
        gui_icon,
    );
    def(
        heap,
        "gui-focus",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        gui_focus,
    );
    def(
        heap,
        "gui-grab-cursor",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        gui_grab_cursor,
    );
    def(
        heap,
        "gui-fullscreen!",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        gui_fullscreen,
    );
    def(
        heap,
        "gui-maximize!",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        gui_maximize,
    );
    def(
        heap,
        "gui-size",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        gui_size,
    );
    def(
        heap,
        "gui-held-key",
        Arity::exact(1),
        Sig::new(vec![int], string.union(kw).union(nil_ty)),
        gui_held_key,
    );
    def(
        heap,
        "gui-draw",
        Arity::exact(2),
        Sig::new(vec![int, vec_ty], nil_ty),
        gui_draw,
    );
    // The font seam: a global default cell font (`gui-font!`) and runtime family
    // registration (`gui-font-register`); a face's `:family`/`:italic` then select
    // per-section, within the fixed cell grid. (gui feature; error without it.)
    def(
        heap,
        "gui-font!",
        // (gui-font! spec) or (gui-font! id spec): arg 0 is a window id (int) or the
        // spec map; the optional arg 1 is the spec map when an id leads.
        Arity::range(1, 2),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Map]), map_ty], nil_ty),
        gui_font,
    );
    def(
        heap,
        "gui-font-register",
        Arity::exact(2),
        Sig::new(vec![kw, map_ty], kw),
        gui_font_register,
    );
    // The window content inset (`gui-inset!`): a blank pixel margin before the cell
    // grid on every edge, so a GUI app's text breathes instead of sitting flush.
    def(
        heap,
        "gui-inset!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Float])], nil_ty),
        gui_inset,
    );
    // The one process-introspection accessor the language can't reach from Brood
    // (the mailbox queue lives behind the scheduler registry). Everything else an
    // observer shows — pid id, liveness — is assembled in Brood (std/observer.blsp).
    def(
        heap,
        "mailbox-size",
        Arity::exact(1),
        Sig::new(vec![pid_ty], int.union(nil_ty)),
        mailbox_size,
    );
    // `(process-info pid)` — an Erlang-`process_info`-style snapshot map for a
    // live local process (nil for remote/dead), the introspection surface a
    // process observer/debugger reads. Assembled in Rust because every field is
    // kernel-internal (registry / scheduler / monitor tables). ADR-051.
    def(
        heap,
        "process-info",
        Arity::exact(1),
        Sig::new(vec![pid_ty], map_ty.union(nil_ty)),
        process_info,
    );

    // type reflection — the tag predicates (nil?/int?/string?/…) are Brood
    // (std/prelude.blsp) over this one reflective primitive.
    def(
        heap,
        "type-of",
        Arity::exact(1),
        Sig::new(vec![any], kw),
        type_of,
    );

    // value <-> text and I/O
    def(
        heap,
        "str",
        Arity::any(),
        Sig::variadic(any, string),
        str_concat,
    );
    def(
        heap,
        "%string-join",
        Arity::exact(2),
        Sig::new(vec![string, seq], string),
        string_join,
    );
    def(
        heap,
        "pr-str",
        Arity::exact(1),
        Sig::new(vec![any], string),
        pr_str,
    );
    def(
        heap,
        "print",
        Arity::any(),
        Sig::variadic(any, nil_ty),
        print,
    );
    def(
        heap,
        "eprint",
        Arity::any(),
        Sig::variadic(any, nil_ty),
        eprint,
    );
    // The render/write split behind the dynamic `*out*`/`*err*` ports
    // (std/prelude.blsp, std/io.blsp): `%render` produces the text `print` would
    // show, and `%write-out`/`%write-err` write a ready string to stdout/stderr.
    def(
        heap,
        "%render",
        Arity::any(),
        Sig::variadic(any, string),
        render,
    );
    def(
        heap,
        "%write-out",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        write_out,
    );
    def(
        heap,
        "%write-err",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        write_err,
    );
    def(
        heap,
        "read-line",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        read_line,
    );
    // `println` is Brood over `print` (std/prelude.blsp).
    def(
        heap,
        "stdout-tty?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        stdout_tty,
    );
    def(
        heap,
        "stdin-tty?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        stdin_tty,
    );

    // time
    def(heap, "now", Arity::exact(0), Sig::nullary(int), now);
    def(heap, "now-ns", Arity::exact(0), Sig::nullary(int), now_ns);

    // memory
    def(
        heap,
        "mem-bytes",
        Arity::exact(0),
        Sig::nullary(int),
        mem_bytes,
    );
    def(
        heap,
        "mem-peak",
        Arity::exact(0),
        Sig::nullary(int),
        mem_peak,
    );
    def(
        heap,
        "mem-limit",
        Arity::exact(0),
        Sig::nullary(int),
        mem_limit,
    );
    def(
        heap,
        "mem-soft-limit",
        Arity::exact(0),
        Sig::nullary(int),
        mem_soft_limit,
    );
    // GC debug/introspection builtins — dev surface only. A lean `nest release`
    // runtime (`--no-default-features`) omits them so a shipped app carries no
    // debug instrumentation (ADR-038). Their fn defs are gated to match.
    #[cfg(feature = "dev-tools")]
    {
        def(
            heap,
            "gc-stats",
            Arity::exact(0),
            Sig::nullary(map_ty),
            gc_stats,
        );
        def(
            heap,
            "vm-stats",
            Arity::exact(0),
            Sig::nullary(map_ty),
            vm_stats,
        );
        def(
            heap,
            "gc-collect",
            Arity::exact(0),
            Sig::nullary(map_ty),
            gc_collect,
        );
        def(
            heap,
            "runtime-collect",
            Arity::exact(0),
            Sig::nullary(map_ty),
            runtime_collect,
        );
        def(
            heap,
            "gc-trace",
            Arity::range(0, 1),
            Sig::new(vec![any], bool_ty),
            gc_trace,
        );
    }

    // self-hosting — eval/load/etc. take and return arbitrary forms / values.
    def(
        heap,
        "eval",
        Arity::exact(1),
        Sig::new(vec![any], any),
        eval_builtin,
    );
    def(
        heap,
        "read-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        read_string,
    );
    def(
        heap,
        "read-all",
        Arity::exact(1),
        Sig::new(vec![string], any),
        read_all,
    );
    def(
        heap,
        "read-first",
        Arity::exact(1),
        Sig::new(vec![string], any),
        read_first,
    );
    def(
        heap,
        "eval-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        eval_string,
    );
    def(
        heap,
        "%load-string",
        Arity::exact(1),
        Sig::new(vec![string], any),
        load_string,
    );
    // Output-capture surface for the `with-out-str` prelude macro: push/pop a
    // process-scoped capture buffer (the same mechanism the `nest mcp` dispatcher
    // uses; captures nest). Rust = mechanism, the macro = policy.
    def(
        heap,
        "%capture-begin",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        capture_begin,
    );
    def(
        heap,
        "%capture-take",
        Arity::exact(0),
        Sig::new(vec![], any),
        capture_take,
    );
    // CST parse — mechanism for the in-Brood formatter (std/format.blsp); never
    // fails (malformed input becomes [:error "..."] nodes). Returns nested
    // vectors; see `parse_source` for the shape.
    def(
        heap,
        "parse-source",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        parse_source,
    );
    def(
        heap,
        "scan-tokens",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        scan_tokens,
    );
    def(
        heap,
        "span-runs",
        Arity::range(3, 4),
        Sig::with_rest(vec![string, int, any], any, list_ty),
        span_runs,
    );
    def(
        heap,
        "clipboard-get",
        Arity::exact(0),
        Sig::nullary(any),
        clipboard_get,
    );
    def(
        heap,
        "clipboard-set!",
        Arity::exact(1),
        Sig::new(vec![string], string),
        clipboard_set,
    );
    // CST parse with absolute positions — every node a map `{:kind :start :end …}`
    // (char offsets). Backs structural navigation (std/sexp); see
    // `parse_source_positioned` for the shape.
    def(
        heap,
        "parse-source-positioned",
        Arity::exact(1),
        Sig::new(vec![string], map_ty),
        parse_source_positioned,
    );
    // Foreign-language CST via tree-sitter (feature "treesit"), in the SAME node
    // shape as `parse-source-positioned` so std/sexp + the editor modes navigate
    // it unchanged. Always registered; errors if built without the feature. §C.
    def(
        heap,
        "tree-sitter-parse",
        Arity::exact(2),
        Sig::new(vec![string, kw], map_ty),
        tree_sitter_parse,
    );
    def(
        heap,
        "load",
        Arity::exact(1),
        Sig::new(vec![string], any),
        load,
    );
    def(
        heap,
        "reload-defs",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        reload_defs,
    );
    def(
        heap,
        "%builtin-module",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        builtin_module,
    );
    def(
        heap,
        "%builtin-doc",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        builtin_doc,
    );
    // Release-bundle mechanism (ADR-038): an app produced by `nest release`
    // carries its source appended to the binary. These let `std/project.blsp`
    // boot it; `%builtin-module` (above) already consults the bundle, so
    // `require` resolves an app's modules with no load-path change.
    def(
        heap,
        "%bundled?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        bundled_p,
    );
    def(
        heap,
        "%bundle-manifest",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        bundle_manifest,
    );
    def(
        heap,
        "%bundle-module-names",
        Arity::exact(0),
        Sig::nullary(list_ty),
        bundle_module_names,
    );
    // `apply`'s last positional arg must be a sequence (it's spliced); the
    // intermediate args can be anything. The `Sig` algebra can express
    // "prefix + repeating tail" but not "the *last* item of the tail is
    // special", so the Sig is `(callable, ...any) -> any` — the closest
    // honest approximation. The sequence-at-tail constraint is checked at
    // call time by `apply_builtin` via `heap.seq_items(args[last])`, which
    // surfaces a `wrong_type` error if the last arg isn't a seq. So the
    // Sig is loose, but the runtime is tight.
    def(
        heap,
        "apply",
        Arity::at_least(2),
        Sig::with_rest(vec![callable], any, any),
        apply_builtin,
    );

    // symbols
    def(
        heap,
        "name",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string),
        name_of,
    );
    def(
        heap,
        "symbol",
        Arity::exact(1),
        Sig::new(vec![string.union(sym).union(kw)], sym),
        to_symbol,
    );
    def(
        heap,
        "keyword",
        Arity::exact(1),
        Sig::new(vec![string.union(sym).union(kw)], kw),
        to_keyword,
    );

    // filesystem — mechanism for the Brood module system + project test runner
    def(heap, "cwd", Arity::exact(0), Sig::nullary(string), cwd);
    def(
        heap,
        "file-exists?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        file_exists,
    );
    def(
        heap,
        "dir?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        is_dir,
    );
    def(
        heap,
        "list-dir",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        list_dir,
    );
    def(
        heap,
        "make-dir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        make_dir,
    );
    def(
        heap,
        "spit",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        spit,
    );
    def(
        heap,
        "slurp",
        Arity::exact(1),
        Sig::new(vec![string], string),
        slurp,
    );
    def(
        heap,
        "file-mtime",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        file_mtime,
    );
    def(
        heap,
        "file-size",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        file_size,
    );
    def(
        heap,
        "file-stat",
        Arity::exact(1),
        Sig::new(vec![string], map_ty.union(nil_ty)),
        file_stat,
    );
    def(
        heap,
        "delete-file",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        delete_file,
    );
    def(
        heap,
        "delete-dir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        delete_dir,
    );
    def(
        heap,
        "rename-file",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        rename_file,
    );
    def(
        heap,
        "copy-file",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        copy_file,
    );
    // The one hashing primitive (ADR-037): SHA-256 of a string's bytes → hex.
    // Per-file and directory-tree hashing for the package manager are Brood over
    // this + `slurp`/`list-dir` (std/package.blsp), not a directory primitive.
    def(
        heap,
        "%sha256",
        Arity::exact(1),
        Sig::new(vec![string], string),
        sha256_hex,
    );
    // SHA-256 of a byte vector (HMAC construction and other binary hashing).
    // Counterpart to %sha256 which takes a string; this takes a vector of ints.
    def(
        heap,
        "%sha256-bytes",
        Arity::exact(1),
        Sig::new(vec![any], string),
        sha256_hex_bytes,
    );
    // SHA-1, SHA-384, SHA-512, and MD5 — string and byte-vector variants.
    // String variants hash UTF-8 bytes; -bytes variants hash arbitrary byte ints.
    def(
        heap,
        "%sha1",
        Arity::exact(1),
        Sig::new(vec![string], string),
        sha1_hex,
    );
    def(
        heap,
        "%sha1-bytes",
        Arity::exact(1),
        Sig::new(vec![any], string),
        sha1_hex_bytes,
    );
    def(
        heap,
        "%sha384",
        Arity::exact(1),
        Sig::new(vec![string], string),
        sha384_hex,
    );
    def(
        heap,
        "%sha384-bytes",
        Arity::exact(1),
        Sig::new(vec![any], string),
        sha384_hex_bytes,
    );
    def(
        heap,
        "%sha512",
        Arity::exact(1),
        Sig::new(vec![string], string),
        sha512_hex,
    );
    def(
        heap,
        "%sha512-bytes",
        Arity::exact(1),
        Sig::new(vec![any], string),
        sha512_hex_bytes,
    );
    def(
        heap,
        "%md5",
        Arity::exact(1),
        Sig::new(vec![string], string),
        md5_hex,
    );
    def(
        heap,
        "%md5-bytes",
        Arity::exact(1),
        Sig::new(vec![any], string),
        md5_hex_bytes,
    );
    // HMAC primitives — one-call Rust implementations over the hmac crate already
    // present for the node handshake. Replaces the pure-Brood RFC 2104 construction
    // in std/hash.blsp which was ~200x slower due to hex-encode/decode round-trips.
    def(
        heap,
        "%hmac-sha256",
        Arity::exact(2),
        Sig::new(vec![string, string], string),
        hmac_sha256_fn,
    );
    def(
        heap,
        "%hmac-sha1",
        Arity::exact(2),
        Sig::new(vec![string, string], string),
        hmac_sha1_fn,
    );
    def(
        heap,
        "%hmac-sha512",
        Arity::exact(2),
        Sig::new(vec![string, string], string),
        hmac_sha512_fn,
    );
    // The package manager's git mechanism (ADR-037): resolve a ref to a commit,
    // and clone+checkout a pinned commit. Thin shell-outs to `git`; the cache
    // layout / lock file / conflict policy are all Brood (std/package.blsp).
    def(
        heap,
        "%git-resolve-ref",
        Arity::exact(2),
        Sig::new(vec![string, string], string.union(nil_ty)),
        git_resolve_ref,
    );
    def(
        heap,
        "%git-clone",
        Arity::exact(4),
        Sig::new(vec![string, string, string, string], kw),
        git_clone,
    );
    // Delete a cached dependency tree. Bounded to paths under `_deps/` — refuses
    // anything else, so a mis-pathed `nest update` can't rm the wrong directory.
    def(
        heap,
        "%rm-rf",
        Arity::exact(1),
        Sig::new(vec![string], kw),
        rm_rf,
    );

    // system / environment
    def(
        heap,
        "getenv",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        getenv,
    );
    def(
        heap,
        "hostname",
        Arity::exact(0),
        Sig::nullary(string),
        hostname,
    );
    def(
        heap,
        "run-process",
        Arity::exact(2),
        Sig::new(vec![string, seq], int),
        run_process,
    );
    def(
        heap,
        "%env-all",
        Arity::exact(0),
        Sig::nullary(map_ty),
        env_all,
    );
    def(
        heap,
        "%argv",
        Arity::exact(0),
        Sig::nullary(seq),
        argv_builtin,
    );
    def(
        heap,
        "%os-type",
        Arity::exact(0),
        Sig::nullary(kw),
        os_type_builtin,
    );
    def(
        heap,
        "%os-cmd",
        Arity::at_least(1),
        Sig::new(vec![string, seq], map_ty),
        os_cmd,
    );
    def(
        heap,
        "%halt",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        halt_builtin,
    );

    // macros
    def(
        heap,
        "macroexpand-1",
        Arity::exact(1),
        Sig::new(vec![any], any),
        macroexpand_1,
    );
    // `macroexpand` (the fixpoint loop) is written in Brood (`std/prelude.blsp`)
    // over this single-step primitive — ADR-064, so its loop state is auto-rooted
    // rather than hand-rooted in Rust. `macros::macroexpand` (Rust) stays for the
    // compile pass, which runs under MACRO_BLOCK.
    // gensym accepts anything as a prefix (string/sym/keyword/nil/anything is
    // turned into its `display` form), so its prefix slot is `any` — not the
    // narrower `string` the original Sig claimed, which made the checker warn
    // on legitimate `(gensym 'foo)` calls.
    def(
        heap,
        "gensym",
        Arity::range(0, 1),
        Sig::new(vec![any], sym),
        gensym,
    );

    // advisory type checker (the Ty lattice's first consumer; see docs/types.md)
    def(
        heap,
        "check",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        check_builtin,
    );
    def(
        heap,
        "check-file",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        check_file_builtin,
    );
    def(
        heap,
        "check-file-structured",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        check_file_structured,
    );
    def(
        heap,
        "check-string-structured",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        check_string_structured,
    );

    // source positions (editor tooling; see docs/tooling.md)
    def(
        heap,
        "form-pos",
        Arity::exact(1),
        Sig::new(vec![any], vec_ty.union(nil_ty)),
        form_pos,
    );
    def(
        heap,
        "current-file",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        current_file,
    );
    def(
        heap,
        "source-location",
        Arity::exact(1),
        Sig::new(vec![sym], vec_ty.union(nil_ty)),
        source_location,
    );
    def(
        heap,
        "references-in-source",
        Arity::exact(2),
        Sig::new(vec![sym.union(string), string], any),
        references_in_source,
    );

    // introspection (editor tooling; see docs/lsp.md) — derive what we can from
    // the bound value (arglist, doc); enumerate the global table for completion.
    def(
        heap,
        "doc",
        Arity::exact(1),
        Sig::new(vec![any], string.union(nil_ty)),
        doc,
    );
    def(
        heap,
        "arglist",
        Arity::exact(1),
        Sig::new(vec![any], list_ty),
        arglist,
    );
    def(
        heap,
        "global-names",
        Arity::exact(0),
        Sig::nullary(list_ty),
        global_names,
    );
    def(
        heap,
        "special-forms",
        Arity::exact(0),
        Sig::nullary(list_ty),
        special_forms,
    );
    def(
        heap,
        "bound?",
        Arity::exact(1),
        Sig::new(vec![sym], bool_ty),
        bound_p,
    );

    // errors / control
    def(
        heap,
        "throw",
        Arity::exact(1),
        Sig::new(vec![any], Ty::NEVER),
        throw,
    );
    // `%force-panic` — deliberately panics the Rust thread when called. Exists
    // *only* in debug builds: it gives the MCP-host panic-isolation regression
    // test a reliable trigger without adding a "intentionally crash" knob to
    // the release surface. `cargo test` (and `nest test` against a debug
    // binary) sees it; `--release` binaries don't.
    #[cfg(debug_assertions)]
    def(
        heap,
        "%force-panic",
        Arity::range(0, 1),
        Sig::new(vec![any], Ty::NEVER),
        force_panic,
    );
    // Shared-blob inspection primitives — debug-only because they leak the
    // representation (a raw pointer) and because they only exist to assert
    // identity / leak-freedom across processes in the blob-share test. Both
    // return `nil` for an inline string or a non-LOCAL handle (PRELUDE/RUNTIME).
    #[cfg(debug_assertions)]
    def(
        heap,
        "%blob-ptr",
        Arity::exact(1),
        Sig::new(vec![string], Ty::ANY),
        blob_ptr,
    );
    #[cfg(debug_assertions)]
    def(
        heap,
        "%blob-strong-count",
        Arity::exact(1),
        Sig::new(vec![string], Ty::ANY),
        blob_strong_count,
    );
    def(
        heap,
        "%try",
        Arity::exact(2),
        Sig::new(vec![callable, callable], any),
        try_catch,
    );
    def(
        heap,
        "%make-macro",
        Arity::exact(1),
        Sig::new(vec![callable], any),
        make_macro,
    );
    def(
        heap,
        "%isolate",
        Arity::exact(1),
        Sig::new(vec![callable], any),
        isolate,
    );

    // dynamic variables (the `defdyn`/`binding` surface is Brood — see prelude)
    def(
        heap,
        "%declare-dynamic",
        Arity::exact(1),
        Sig::new(vec![sym], nil_ty),
        declare_dynamic,
    );
    // Namespaces (ADR-065): `%in-ns` sets the namespace being compiled into. The
    // `ns` macro (prelude) emits it; the resolver pass reads `heap.compile_ns`.
    def(
        heap,
        "%in-ns",
        Arity::exact(1),
        Sig::new(vec![sym], sym),
        in_ns,
    );
    def(
        heap,
        "current-ns",
        Arity::exact(0),
        Sig::new(vec![], sym),
        current_ns,
    );
    // `(%refer 'mod subset)` — populate the current file's import table from a
    // `(:use …)` clause. `subset` is nil (refer all public names) or a seq of
    // bare symbols. The `ns` macro emits it after `(require 'mod)`.
    def(
        heap,
        "%refer",
        Arity::exact(2),
        Sig::new(vec![sym, any], nil_ty),
        refer,
    );
    // `(:alias mod [:as short])` lowers to this — register a module alias so a later
    // `short/name` reference resolves to `mod/name`.
    def(
        heap,
        "%alias",
        Arity::exact(2),
        Sig::new(vec![sym, sym], nil_ty),
        alias,
    );
    // `%binding`'s first arg is the *list/vector of names*, second is the
    // *list/vector of values*, third is the thunk — the macro `binding` emits
    // these as `(quote (*a* *b* …))` + `[v1 v2 …]` + `(fn () …)`.
    def(
        heap,
        "%binding",
        Arity::exact(3),
        Sig::new(vec![seq, seq, callable], any),
        binding,
    );
    def(
        heap,
        "dynamic?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        dynamic_p,
    );

    // processes (concurrency)
    def(
        heap,
        "%spawn",
        Arity::exact(1),
        Sig::new(vec![callable], pid_ty),
        spawn,
    );
    def(
        heap,
        "%spawn-named",
        Arity::exact(2),
        Sig::new(vec![sym.union(kw), callable], pid_ty),
        spawn_named,
    );
    // `send`'s target is a pid OR a `{:name :node}` address map.
    def(
        heap,
        "send",
        Arity::exact(2),
        Sig::new(vec![pid_ty.union(map_ty), any], nil_ty),
        send,
    );
    // Arg shape: (matcher: callable, timeout: int|nil, on-timeout: callable|nil).
    // The `receive` macro in `std/prelude.blsp` expands to exactly this; the
    // `callable|nil` on the third position is for the no-`after`-clause case
    // (the macro passes `nil`).
    def(
        heap,
        "%receive",
        Arity::exact(3),
        Sig::new(
            vec![callable, int.union(nil_ty), callable.union(nil_ty)],
            any,
        ),
        receive_match,
    );
    def(
        heap,
        "self",
        Arity::exact(0),
        Sig::nullary(pid_ty),
        self_pid,
    );
    def(heap, "ref", Arity::exact(0), Sig::nullary(ref_ty), make_ref);
    // `(exit pid reason)` — send an exit signal (Erlang `exit/2`). `:kill` is the
    // untrappable hard kill; any other reason is the soft (next-`receive`) signal.
    def(
        heap,
        "exit",
        Arity::exact(2),
        Sig::new(vec![pid_ty, any], nil_ty),
        exit_proc,
    );
    // `monitor` also accepts a name map (forwarded to the remote node).
    def(
        heap,
        "monitor",
        Arity::exact(1),
        Sig::new(vec![pid_ty.union(map_ty)], ref_ty),
        monitor,
    );
    def(
        heap,
        "demonitor",
        Arity::exact(1),
        Sig::new(vec![ref_ty], nil_ty),
        demonitor,
    );
    // Links (ADR-077): symmetric failure coupling + `trap_exit`, the bidirectional
    // cousin of `monitor`. `link`/`unlink` couple the current process to a pid;
    // `trap-exit` turns a linked peer's death into a `[:EXIT pid reason]` message.
    def(
        heap,
        "link",
        Arity::exact(1),
        Sig::new(vec![pid_ty], nil_ty),
        link_proc,
    );
    def(
        heap,
        "unlink",
        Arity::exact(1),
        Sig::new(vec![pid_ty], nil_ty),
        unlink_proc,
    );
    def(
        heap,
        "trap-exit",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        trap_exit_proc,
    );
    def(
        heap,
        "spawn-count",
        Arity::exact(0),
        Sig::nullary(int),
        spawn_count,
    );
    def(
        heap,
        "peak-threads",
        Arity::exact(0),
        Sig::nullary(int),
        peak_threads,
    );
    def(
        heap,
        "worker-threads",
        Arity::exact(0),
        Sig::nullary(int),
        worker_threads,
    );
    def(
        heap,
        "steal-count",
        Arity::exact(0),
        Sig::nullary(int),
        steal_count,
    );
    def(
        heap,
        "list-processes",
        Arity::exact(0),
        Sig::nullary(list_ty),
        list_processes,
    );

    // distributed nodes (connect two runtimes over TCP — crate::dist)
    def(
        heap,
        "%node-listen",
        Arity::exact(3),
        Sig::new(vec![sym, string, string], sym),
        node_listen,
    );
    def(
        heap,
        "%node-also-listen",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        node_also_listen,
    );
    def(
        heap,
        "%node-connect",
        Arity::exact(2),
        Sig::new(vec![sym, string], sym),
        node_connect,
    );
    def(
        heap,
        "random-token",
        Arity::exact(1),
        Sig::new(vec![int], string),
        random_token,
    );
    def(
        heap,
        "%random-bytes",
        Arity::exact(1),
        Sig::new(vec![int], seq),
        random_bytes,
    );
    def(
        heap,
        "%chacha20-encrypt",
        Arity::exact(3),
        Sig::new(vec![any, any, any], seq),
        chacha20_encrypt,
    );
    def(
        heap,
        "%chacha20-decrypt",
        Arity::exact(3),
        Sig::new(vec![any, any, any], any),
        chacha20_decrypt,
    );
    def(
        heap,
        "%pbkdf2-sha256",
        Arity::exact(4),
        Sig::new(vec![string, string, int, int], seq),
        pbkdf2_sha256_fn,
    );
    def(
        heap,
        "spit-private",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        spit_private,
    );
    def(
        heap,
        "register",
        Arity::exact(2),
        // A name may be a symbol OR a keyword — `expect_node_name` accepts both, and
        // `:name` lookups in `send`/`node-name` use keywords, so the sig must too.
        Sig::new(vec![sym.union(kw), pid_ty], pid_ty),
        register_name,
    );
    def(
        heap,
        "whereis",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw)], pid_ty.union(nil_ty)),
        whereis_name,
    );
    // `node-name` is the keyword `:nonode` until `node-start` sets it to a symbol.
    def(
        heap,
        "node-name",
        Arity::exact(0),
        Sig::nullary(sym.union(kw)),
        node_name,
    );
    def(heap, "nodes", Arity::exact(0), Sig::nullary(list_ty), nodes);
    def(
        heap,
        "monitor-node",
        Arity::exact(1),
        // A node name may be a symbol OR a keyword — `node-name`/`connect` return
        // the authoritative `:name@host` as a keyword, so monitoring it must not
        // warn (matches `register`/`whereis`; `expect_node_name` accepts both).
        Sig::new(vec![sym.union(kw)], ref_ty),
        monitor_node,
    );
    def(
        heap,
        "demonitor-node",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw)], nil_ty),
        demonitor_node,
    );
    def(
        heap,
        "disconnect",
        Arity::exact(1),
        // Same name domain as `monitor-node`: the authoritative `:name@host`
        // keyword `connect`/`nodes` hand back (or a symbol).
        Sig::new(vec![sym.union(kw)], bool_ty),
        disconnect,
    );
}

/// Docstrings + parameter names for the public primitives, so `(doc 'name)`,
/// `(arglist 'name)`, and LSP hover treat a Rust builtin like a Brood `defn`
/// (which can't apply here — primitives have no source body). One row per
/// user-facing primitive; mirrors the "Purpose" column of `docs/primitives.md`.
/// `&` in the params marks a rest (variadic) tail. Internal `%`-prefixed
/// primitives are intentionally absent (they aren't meant to be called directly).
#[rustfmt::skip]
static PRIMITIVE_DOCS: &[(&str, &[&str], &str)] = &[
    ("rem", &["a", "b"], "Integer remainder of a / b (truncated, taking the sign of the dividend)."),
    ("floor", &["x"], "Round x toward negative infinity to an integer."),
    ("bit-and", &["a", "b"], "Bitwise AND of integers a and b."),
    ("bit-or", &["a", "b"], "Bitwise (inclusive) OR of integers a and b."),
    ("bit-xor", &["a", "b"], "Bitwise exclusive-OR of integers a and b."),
    ("bit-not", &["a"], "Bitwise complement of integer a (two's-complement, so (bit-not n) = (- (- n) 1))."),
    ("bit-shift-left", &["a", "n"], "Shift integer a left by n bits (0 <= n < 64); bits shifted past bit 63 are discarded."),
    ("bit-shift-right", &["a", "n"], "Arithmetic (sign-preserving) right shift of integer a by n bits (0 <= n < 64)."),
    ("bit-count", &["a"], "Population count: the number of 1 bits in integer a's two's-complement representation (a negative a counts its sign bits, so (bit-count -1) = 64). For a bignum it is the popcount of the magnitude."),
    ("bit-positions", &["a"], "A vector of the 0-based bit indices set in non-negative integer a, ascending (e.g. (bit-positions 6) = [1 2]). O(number of set bits) — for a bignum it scans the magnitude. The inverse of summing (bit-shift-left 1 i); handy for enumerating a bitset's members."),
    ("cons", &["x", "xs"], "A new pair with head x and tail xs."),
    ("first", &["coll"], "The head of a list or vector, or nil if empty."),
    ("rest", &["coll"], "All but the head of a list or vector."),
    ("range?", &["x"], "True if x is a lazy range (as produced by range). Ranges fold/reduce/sum/count without materialising; other ops treat them as the list they stand for."),
    ("vector", &["&", "items"], "A vector of the given items."),
    ("vector-ref", &["v", "i"], "The element at index i of vector v."),
    ("vector-length", &["v"], "The number of elements in vector v."),
    ("vector-assoc", &["v", "i", "x"], "A fresh vector like v with index i (in [0, len)) set to x."),
    ("subvec", &["v", "start", "end"], "A fresh vector of v's elements in [start, end); end defaults to the length."),
    ("compare", &["a", "b"], "Structural total-order comparison: -1 if a sorts before b, 0 if equal, 1 if after. Numbers numerically; strings/keywords/symbols by text; vectors/lists lexicographically; cross-kind by a stable tag rank. The binary form of `sort`'s order — `sort-by` and custom comparators build on it."),
    ("hash-map", &["&", "kvs"], "A map from alternating key/value arguments (last wins on duplicate keys)."),
    ("map-get", &["m", "k", "default"], "The value at key k in map m, or default (else nil)."),
    ("map-assoc", &["m", "k", "v"], "A fresh map like m with key k set to v."),
    ("map-dissoc", &["m", "k"], "A fresh map like m with key k removed."),
    ("map-pairs", &["m"], "The entries of m as a list of [k v] vectors, in insertion order."),
    ("map-count", &["m"], "The number of entries in map m. O(1) — the CHAMP root tracks its size."),
    ("transient", &["m"], "Open a transient (mutable-while-building) copy of map m for fast assoc!/dissoc!, à la Clojure. Build it up with assoc!/dissoc! (each mutates and returns the same handle), then call persistent! to get an immutable map back. Building a big map this way is ~an order of magnitude cheaper than folding assoc over a persistent map (no path-copy per step)."),
    ("assoc!", &["t", "k", "v"], "Set key k to v in transient t, mutating it in place and returning the same transient. Errors after persistent!. The fast counterpart to assoc on a persistent map."),
    ("dissoc!", &["t", "k"], "Remove key k from transient t in place, returning the same transient. Errors after persistent!."),
    ("persistent!", &["t"], "Close transient t and return its contents as a normal immutable map. After this, any assoc!/dissoc!/persistent! on t errors."),
    ("transient?", &["x"], "True if x is a transient map (from transient), live or already persistent!-ed."),
    ("transient-get", &["t", "k", "default"], "The value at key k in live transient t, or default (else nil). Lookups are allowed on a live transient (Clojure-style); errors after persistent!. The kernel hook get dispatches to for a transient."),
    ("transient-count", &["t"], "The number of entries in live transient t, O(1). The kernel hook count dispatches to for a transient."),
    ("transient-contains?", &["t", "k"], "True if live transient t has key k. The kernel hook contains? dispatches to for a transient."),
    ("string-length", &["s"], "The number of characters in string s."),
    ("display-width", &["s"], "How many terminal/grid cells string s occupies (grapheme-cluster aware: an emoji / flag / CJK char counts as 2, a combining mark 0). The width-aware counterpart to string-length."),
    ("substring", &["s", "start", "end"], "The characters of s in the range [start, end), char-indexed. end is optional and defaults to (string-length s), so (substring s start) is \"from start to the end\"."),
    ("string-split", &["s", "sep"], "Split s into a list of substrings on each occurrence of sep, in one O(n) pass. An empty separator splits s into its individual characters."),
    ("string-span", &["s", "start", "chars"], "The char index just past the maximal run of chars (a set, given as a string) starting at char `start` in s — `start` itself if the char there isn't in the set. The forward char-class scan a tokenizer skips a whitespace/digit run with; O(run) native. See also string-span-until."),
    ("string-span-until", &["s", "start", "chars"], "The char index of the first char of s in the set `chars` (a string) at or after char `start`, or (string-length s) if none — the maximal run of chars NOT in the set. For scanning up to a delimiter (comment-to-newline, atom-to-delimiter). The complement of string-span."),
    ("upper", &["s"], "s upper-cased (Unicode-aware)."),
    ("lower", &["s"], "s lower-cased (Unicode-aware)."),
    ("char->int", &["s"], "Unicode codepoint of the first character of string s (identical to the byte value for ASCII)."),
    ("int->char", &["n"], "A 1-char string for Unicode codepoint n. Errors on an invalid codepoint."),
    ("string->utf8-bytes", &["s"], "The UTF-8 encoding of s as a vector of byte integers (0–255)."),
    ("utf8-bytes->string", &["bytes"], "Decode a vector of UTF-8 byte integers (0–255) into a string. Errors on invalid UTF-8."),
    ("to-fixed", &["x", "n"], "Render number x as a string with exactly n digits after the decimal point (rounded). n must be >= 0."),
    ("string->number", &["s"], "Parse s strictly as an int (a bignum when out of i64 range), else a float, else nil (unlike read-string). The inverse of number->string."),
    ("sin",   &["x"], "The sine of x (radians). Returns a float."),
    ("cos",   &["x"], "The cosine of x (radians). Returns a float."),
    ("tan",   &["x"], "The tangent of x (radians). Returns a float."),
    ("asin",  &["x"], "The arcsine of x in radians. x must be in [-1, 1]; raises otherwise."),
    ("acos",  &["x"], "The arccosine of x in radians. x must be in [-1, 1]; raises otherwise."),
    ("atan",  &["x"], "The arctangent of x in radians (result in [-π/2, π/2])."),
    ("atan2", &["y", "x"], "The angle in radians of the vector (x, y) from the positive x-axis, in (-π, π]. Handles x=0."),
    ("exp",   &["x"], "e raised to the power x. Returns a float."),
    ("ln",    &["x"], "The natural logarithm of x. x must be positive; raises otherwise."),
    ("log2",  &["x"], "The base-2 logarithm of x. x must be positive; raises otherwise."),
    ("log10", &["x"], "The base-10 logarithm of x. x must be positive; raises otherwise."),
    ("string->rope", &["s"], "A rope (editor buffer text) holding the characters of string s."),
    ("rope->string", &["r"], "The full text of rope r as a string."),
    ("rope-length", &["r"], "The number of characters in rope r."),
    ("rope-line-count", &["r"], "The number of lines in rope r (a trailing newline ends a line; \"\" is 1 line)."),
    ("rope-insert", &["r", "idx", "s"], "A fresh rope with string s inserted at character index idx."),
    ("rope-delete", &["r", "start", "end"], "A fresh rope with characters [start, end) removed."),
    ("rope-slice", &["r", "start", "end"], "The text of characters [start, end) of rope r, as a string."),
    ("rope-line", &["r", "n"], "The text of line n (0-based) of rope r, including any trailing newline."),
    ("rope-char->line", &["r", "idx"], "The 0-based line index containing character idx."),
    ("rope-line->char", &["r", "n"], "The character index where line n (0-based) begins."),
    ("tcp-connect", &["host", "port"], "Connect to host:port; inbound data is delivered to the calling process as [:tcp sock data] / [:tcp-closed sock] messages. Returns a socket. Throws on failure."),
    ("tcp-listen", &["host", "port"], "Bind a listening socket on host:port (port 0 = OS-assigned); connections arrive as [:tcp-accept lsock client] messages to the calling process. Returns a socket."),
    ("tls-request", &["host", "port", "request"], "Make one HTTPS request to host:port (TLS): the response arrives at the calling process as [:tcp sock data] … [:tcp-closed sock] messages (or [:tcp-error sock msg]). Returns a socket id; pair with tcp-drain. Low-level — prefer http-get."),
    ("tcp-send", &["sock", "s"], "Write the whole string s to sock (blocking). Text mode (default): s is sent as UTF-8. Binary mode (see tcp-set-binary): each codepoint of s must be 0–255 and is written as one raw byte. Returns nil; throws on error."),
    ("tcp-set-binary", &["sock", "on"], "Switch sock between text mode (default) and binary mode. In binary mode inbound [:tcp …] data is a byte-faithful Latin-1 string (one codepoint 0–255 per byte received) and tcp-send writes each codepoint 0–255 as one raw byte — for length-prefixed / control-byte protocols like WebSocket framing. Returns nil; throws if sock is gone or a listener."),
    ("tcp-controlling-process", &["sock", "pid"], "Make pid the owner of sock's inbound data: starts reading a just-accepted (passive) socket, or retargets an active one. Returns nil."),
    ("tcp-close", &["sock"], "Close sock (a stream or listener), releasing its fd / stopping its accept loop. Idempotent; returns nil."),
    ("tcp-local-port", &["sock"], "The local port sock is bound to, or nil."),
    ("proc-spawn", &["prog", "args", "opts"], "Spawn prog (a string) with args (a list/vector of strings) as a persistent child process with piped stdio. An optional opts map tunes the child: :cwd (a string) sets its working directory, :env (a map of string->string) adds environment variables on top of the inherited environment. Its stdout/stderr arrive at the calling process as [:proc handle data] / [:proc-err handle data] messages, and [:proc-closed handle code] on exit (code is the exit status, or nil if signalled). Returns a subprocess handle. Throws if prog can't be spawned."),
    ("proc-send", &["p", "s"], "Write the whole string s to subprocess p's stdin (blocking) and flush. Returns nil; throws if p is unknown/closed."),
    ("proc-close", &["p"], "Terminate subprocess p: kill it if still running and close its stdin. Idempotent; returns nil. The final [:proc-closed handle code] still arrives at the owner."),
    ("table", &[], "Create a new empty in-memory table (Brood's ETS): a shared, mutable key→value store behind an opaque handle. Unlike a map it is mutated in place (table-put/table-delete) and shared by identity — the handle can be sent to other processes, which all see the same store. Stores deep clones (keys/values are copied in and out), so no two processes alias a stored value. Local to this runtime; not node-portable. Returns the handle."),
    ("table-put", &["t", "k", "v"], "Store v under key k in table t, overwriting any existing entry. Keys use the same structural equality as map keys. Returns t (for threading). Both k and v are deep-copied into the store."),
    ("table-get", &["t", "k", "default"], "A fresh copy of the value stored under k in table t, or default (nil if omitted) when absent."),
    ("table-has?", &["t", "k"], "True if table t has an entry for key k."),
    ("table-delete", &["t", "k"], "Remove key k from table t if present. Returns t."),
    ("table-incr", &["t", "k", "delta"], "Atomically add delta (default 1) to the integer at key k in table t, treating an absent key as 0, and return the new value. The read-modify-write is atomic under the table lock, so concurrent increments never lose an update — use this for counters. Errors if the existing value is not an integer."),
    ("table-count", &["t"], "The number of entries in table t."),
    ("table-snapshot", &["t"], "A consistent point-in-time copy of the whole table t as an immutable map. Atomic; the returned map is unaffected by later mutation of t. Use map ops (keys/vals/get/reduce) on it. O(n)."),
    ("table-drop", &["t"], "Remove table t from the registry, freeing its store. Idempotent; returns true if it existed. Other handles to t then error on use."),
    ("type-of", &["x"], "The runtime type of x as a keyword (:int, :string, :pair, ...)."),
    ("check", &["form"], "Advisory type-check a quoted form: a list of warning strings, or nil. Never raises."),
    ("check-file", &["path"], "Advisory type-check every top-level form in the file at path: a list of `path:line:col: warning: …` strings, or nil. Does not evaluate the file."),
    ("check-file-structured", &["path"], "Like check-file but returns a list of `{:file :line :col :message}` maps instead of GNU-format strings — for tools (the `nest mcp` `check` tool, editor diagnostics)."),
    ("check-string-structured", &["src"], "Advisory type-check the source string `src`, returning a list of `{:line :col :message}` maps (1-based positions), or `()` when `src` doesn't parse (e.g. incomplete input) — the string-source counterpart of check-file-structured, for live editor-buffer diagnostics."),
    ("str", &["&", "xs"], "Concatenate the display forms of the arguments into one string."),
    ("pr-str", &["x"], "The readable (re-readable) text form of x."),
    ("print", &["&", "xs"], "Write the display forms of the arguments to stdout; returns nil."),
    ("eprint", &["&", "xs"], "Write the display forms of the arguments to stderr; returns nil."),
    ("%render", &["&", "xs"], "The space-joined display forms of the arguments as one string (no output). The rendering half of `print`; Brood's print/println route the result through the dynamic `*out*` port."),
    ("%write-out", &["s"], "Write the ready string `s` to the current stdout sink — the active capture buffer (`with-out-str`) if set, else real stdout. The default `*out*` port."),
    ("%write-err", &["s"], "Write the ready string `s` to real stderr (never captured). The default `*err*` port."),
    ("stdout-tty?", &[], "True when stdout is an interactive terminal (false when piped or captured)."),
    ("stdin-tty?", &[], "True when stdin is an interactive terminal (false when redirected from a pipe or file). The REPL gates raw-mode line editing on this."),
    ("now", &[], "Wall-clock milliseconds since the Unix epoch."),
    ("now-ns", &[], "Wall-clock nanoseconds since the Unix epoch (finer-grained than now)."),
    ("mem-bytes", &[], "Bytes currently allocated process-wide."),
    ("mem-peak", &[], "High-water mark of allocated bytes since process start."),
    ("mem-limit", &[], "Hard memory ceiling in bytes (0 = unlimited); crossing it aborts the process. Set via BROOD_MEM_LIMIT."),
    ("mem-soft-limit", &[], "Soft memory ceiling in bytes (0 = unlimited); crossing it raises a catchable E0043 at the next safepoint."),
    ("gc-stats", &[], "A snapshot map of GC activity: :collections, :copied, :reclaimed (cumulative object counts), :live, :live-bytes, :threshold (next-collection trigger) for the caller's own LOCAL heap; :runtime-closures and :runtime-threshold for the *shared* RUNTIME code region (its promoted-closure count + next auto-compact trigger — same for every process); and :debug-build (true if built with debug assertions — not a perf build). The LOCAL figures are per-process; use (runtime-collect) for the RUNTIME live/reclaimable split."),
    ("vm-stats", &[], "A snapshot map of VM work-attribution counters (the perf-stats feature). :enabled is false unless the binary was built with --features perf-stats; when true, process-global cumulative totals: :vm-apply (closure activations), :tail-call/:self-tail (trampoline iterations), :tw-defer (tree-walker fallbacks), :call-ic-hit/:call-ic-miss, :global-ic-hit/:global-ic-miss, :prim2-inline/:prim2-fallback, :prim1-inline/:prim1-fallback, :env-get/:env-hops (lookups + chain frames walked), :alloc (LOCAL allocations). Tells you whether the VM is dispatch-, env-, or alloc-bound. A counting tool, not a timing one — read times from the benches (docs/benchmarking.md)."),
    ("gc-collect", &[], "Force a collection of this process's LOCAL heap now, returning the post-collection gc-stats map. An observability/test aid, not a load-bearing trigger — automatic collection at the eval safepoint already keeps memory bounded."),
    ("runtime-collect", &[], "Compact the shared RUNTIME code region, reclaiming superseded versions of redefined globals (hot-reload churn). Returns {:before N :after M :reclaimed (N-M) :ran bool} (closure counts). Runs only when this runtime is uniquely owned (no other live process) — otherwise :ran is false and nothing changes. Usually unnecessary: the eval safepoint auto-compacts once hot-reload churn crosses a threshold (single-process); this forces it now. ADR-076 follow-up / docs/runtime-collector-exploration.md."),
    ("gc-trace", &["on?"], "Query (no arg) or set (truthy arg) per-collection GC trace logging for this process; returns the resulting state. When on, each minor/major collection prints a one-line summary to stderr. Defaulted from BROOD_GC_TRACE."),
    ("eval", &["form"], "Evaluate a form in the global environment."),
    ("read-string", &["s"], "Parse and return the single form in string s. Errors on trailing content after the form (rather than silently dropping it) — use read-all for input with more than one form."),
    ("read-all", &["s"], "Parse every form in string s and return them as a list (the all-forms sibling of read-string)."),
    ("read-first", &["s"], "Parse and return the first form in string s, ignoring any trailing forms (the lenient sibling of read-string — for peeking a multi-form source's leading form, e.g. a file's (defmodule …) header)."),
    ("parse-source", &["s"], "Parse s into a lossless CST tree as nested vectors (mechanism for std/format.blsp)."),
    ("scan-tokens", &["s"], "Lexically tokenize Brood source s into a vector of [start end kind text] tokens (char offsets, end-exclusive; whitespace skipped). kind is :comment, :string, :number, :keyword, :symbol, :open, or :close. The lossless token stream a fontifier / structural tool walks — the per-char scan runs natively, leaving policy (faces, head-position) to the consumer over O(tokens)."),
    ("span-runs", &["text", "base", "spans", "ranges"], "Tile text (first char at offset base) into a list of [substring face] runs from ascending, non-overlapping [start end face] spans: gaps are nil-faced, each span its text in its face. With optional overlay ranges ([lo hi face], may overlap/be unordered) each char's face is its span face with every covering range face merged on top (later wins). Adjacent equal-face runs coalesce. The highlight span->runs tiler (fontify-runs), in Rust. Faces are opaque maps."),
    ("clipboard-get", &[], "The OS clipboard's text, or nil when empty / non-text / unavailable (no display server, or a build without the clipboard feature)."),
    ("clipboard-set!", &["s"], "Copy string s to the OS clipboard so other apps can paste it; returns s. A no-op (still returns s) when no clipboard is available or the clipboard feature is off."),
    ("parse-source-positioned", &["s"], "Parse s into a CST of maps, each `{:kind :start :end}` (leaves add :text, containers/wrappers add :kids) with half-open character offsets — for structural navigation (std/sexp)."),
    ("tree-sitter-parse", &["source", "lang"], "Parse source (a string) with the tree-sitter grammar named by keyword lang (:ruby, :elixir) into a positioned CST — the SAME node-map shape as parse-source-positioned (`{:kind :start :end :named}`; leaves add :text, nodes with children add :kids), :kind a keyword of the tree-sitter node type and :named false for anonymous tokens (keywords/punctuation). Char offsets, so std/sexp + the editor's fontify navigate it unchanged. Errors on an unknown language, or if the runtime was built without --features treesit."),
    ("eval-string", &["s"], "Read and evaluate every form in string s (the string analogue of load)."),
    ("load", &["path"], "Read and evaluate every form in the file at path."),
    ("reload-defs", &["path"], "Re-evaluate only the def-style top-level forms in `path` (def, defn, defmacro, defmodule, defdyn, …) — skipping other top-level calls. Used by file watchers to refresh code without re-running side-effecting top-level calls like a `(main-loop)`. Returns nil."),
    ("apply", &["f", "&", "args"], "Call f with the leading args plus the final list argument spliced in as trailing args."),
    ("name", &["x"], "The spelling of a symbol or keyword as a string (no leading colon)."),
    ("symbol", &["x"], "Coerce a string, symbol, or keyword to the matching symbol (interning if needed)."),
    ("keyword", &["x"], "Coerce a string, symbol, or keyword to the matching keyword (interning if needed)."),
    ("cwd", &[], "The current working directory."),
    ("file-exists?", &["path"], "Whether path exists."),
    ("dir?", &["path"], "Whether path is a directory."),
    ("list-dir", &["path"], "The entry names directly under directory path, sorted."),
    ("make-dir", &["path"], "Create a directory and any missing parents (like mkdir -p)."),
    ("spit", &["path", "s"], "Write string s to the file at path."),
    ("spit-private", &["path", "s"], "Write string s to path with owner-only (0600) permissions, creating the parent dir if needed. The private-by-default write for a secret (spit leaves a world-readable file)."),
    ("slurp", &["path"], "Read the whole file at path into a string (does not evaluate it)."),
    ("random-token", &["n"], "n cryptographically-strong random bytes from the OS RNG, hex-encoded as a 2n-char string. Used to mint a node cookie."),
    ("%sha256", &["s"], "Lowercase hex SHA-256 of string s's bytes. The package manager's one hashing primitive (ADR-037); file/tree hashing is Brood over it."),
    ("%sha256-bytes", &["bytes"], "Lowercase hex SHA-256 of a vector (or list) of byte integers 0–255. Use this for hashing arbitrary binary data; %sha256 hashes UTF-8 string bytes."),
    ("%sha1",         &["s"],     "Lowercase hex SHA-1 of string s's UTF-8 bytes. NOT collision-resistant; use sha256 for security-sensitive hashing."),
    ("%sha1-bytes",   &["bytes"], "Lowercase hex SHA-1 of a vector (or list) of byte integers 0–255."),
    ("%sha384",       &["s"],     "Lowercase hex SHA-384 of string s's UTF-8 bytes."),
    ("%sha384-bytes", &["bytes"], "Lowercase hex SHA-384 of a vector (or list) of byte integers 0–255."),
    ("%sha512",       &["s"],     "Lowercase hex SHA-512 of string s's UTF-8 bytes."),
    ("%sha512-bytes", &["bytes"], "Lowercase hex SHA-512 of a vector (or list) of byte integers 0–255."),
    ("%md5",          &["s"],     "Lowercase hex MD5 of string s's UTF-8 bytes. NOT collision-resistant; use sha256 for security-sensitive hashing."),
    ("%md5-bytes",    &["bytes"], "Lowercase hex MD5 of a vector (or list) of byte integers 0–255."),
    ("%hmac-sha256", &["key", "message"], "HMAC-SHA256 of `message` keyed with `key` (both strings). Returns lowercase hex. RFC 2104 over sha2."),
    ("%hmac-sha1",   &["key", "message"], "HMAC-SHA1 of `message` keyed with `key`. Returns lowercase hex. Not collision-resistant; prefer hmac-sha256."),
    ("%hmac-sha512", &["key", "message"], "HMAC-SHA512 of `message` keyed with `key`. Returns lowercase hex."),
    ("%git-resolve-ref", &["url", "ref"], "Resolve git `ref` (tag/branch/commit) at remote `url` to a commit hash (via `git ls-remote`), or nil if not found. The package manager's ref-pinning mechanism (ADR-037)."),
    ("%git-clone", &["url", "dest", "ref", "commit"], "Shallow-clone `url` into `dest` and check out the exact `commit` (detached); `ref` is the fetch fallback. Returns :ok or throws. The package manager's fetch mechanism (ADR-037)."),
    ("%rm-rf", &["path"], "Recursively delete `path`. Bounded to paths under `_deps/` (refuses anything else). Idempotent. The package manager's cache-eviction mechanism (ADR-037)."),
    ("read-line", &[], "Read one line from stdin; returns the line as a string (trailing newline stripped) or nil at end of input."),
    ("file-mtime", &["path"], "Last-modified time of path as epoch-milliseconds, or nil if the file is missing. Cheap (stat) — pair with `load` to drive a hot-reloader."),
    ("file-size", &["path"], "Size of the file at path in bytes, or nil if it is missing."),
    ("file-stat", &["path"], "Metadata for path in ONE stat as a map {:dir? :size :mtime :atime :symlink? :exec? :mode :nlink :uid :gid :owner :group}, or nil if missing. :symlink? reads the link itself (lstat); the rest follow it. :mtime/:atime are epoch-ms last-modified/last-access (nil if unreadable; :atime may be coarse under relatime/noatime mounts); :exec? is the owner-execute bit; :mode is the unix permission bits (0 off-unix); :nlink the hard-link count; :uid/:gid the numeric ids; :owner/:group their resolved names (the numeric id as a string if unresolved). Everything an `ls -l` row + a recency sort needs in one syscall."),
    ("delete-file", &["path"], "Remove the file at path. Idempotent (nil if already absent); errors on a real I/O failure."),
    ("delete-dir", &["path"], "Remove a directory and everything under it (recursive). Idempotent (nil if already absent); errors on a real I/O failure."),
    ("rename-file", &["from", "to"], "Rename/move file `from` to `to`. Returns nil; errors on failure."),
    ("copy-file", &["from", "to"], "Copy file `from` to `to` (replacing `to`), preserving contents and permissions. Binary-safe (unlike slurp+spit). Returns nil; errors on failure."),
    ("getenv", &["name"], "The value of environment variable name, or nil if unset."),
    ("hostname", &[], "This machine's short hostname (no domain). Used to qualify a node name as name@host."),
    ("run-process", &["prog", "args"], "Run external program prog with an args list, inheriting stdio; returns its exit code."),
    ("%env-all", &[], "All environment variables as a map of string→string."),
    ("%argv", &[], "Command-line arguments as a vector of strings (including argv[0])."),
    ("%os-type", &[], "The host OS as a keyword: :linux, :macos, or :windows."),
    ("%os-cmd", &["prog", "&", "args"], "Run prog (with optional args list) capturing stdout/stderr; returns {:stdout s :stderr s :exit n}."),
    ("%halt", &["code"], "Terminate the process with exit code. Never returns."),
    ("%random-bytes", &["n"], "n cryptographically-strong random bytes as a vector of ints 0–255."),
    ("%chacha20-encrypt", &["key-bytes", "nonce-bytes", "plaintext-bytes"], "Encrypt plaintext-bytes with ChaCha20-Poly1305 (AEAD). key-bytes must be 32 bytes; nonce-bytes must be 12 bytes. Returns ciphertext bytes (plaintext + 16-byte auth tag)."),
    ("%chacha20-decrypt", &["key-bytes", "nonce-bytes", "ciphertext-bytes"], "Decrypt ciphertext-bytes with ChaCha20-Poly1305. Returns plaintext bytes, or :error if authentication fails."),
    ("%pbkdf2-sha256", &["password", "salt", "iterations", "key-len"], "PBKDF2-HMAC-SHA256 key derivation. Returns a key-len-byte vector. Use iterations >= 600000 for password storage."),
    ("macroexpand-1", &["form"], "Expand form by a single macro step."),
    // `macroexpand` is a Brood prelude fn (ADR-064), documented via its docstring.
    ("gensym", &["prefix"], "A fresh, unique symbol, with an optional name prefix."),
    ("form-pos", &["form"], "A form's [line col] source position, or nil."),
    ("current-file", &[], "The path of the file currently being loaded, or nil."),
    ("source-location", &["name"], "Where global name was defined, as [file line col], or nil. Quote it: (source-location 'foo)."),
    ("references-in-source", &["name", "source"], "Occurrences of the global `name` in `source`, as a list of [line col] (1-based); locals that shadow it are excluded."),
    ("doc", &["f"], "The docstring of a function, macro, or primitive, or nil."),
    ("arglist", &["f"], "The parameter list of a function, macro, or primitive, or nil."),
    ("global-names", &[], "Every globally bound symbol, sorted by spelling."),
    ("special-forms", &[], "The special-form / core-macro names (strings) that read as keywords — the canonical list shared by the syntax highlighter and the LSP."),
    ("bound?", &["sym"], "Whether sym is bound in scope. Quote it: (bound? 'foo)."),
    ("dynamic?", &["x"], "Whether x is a symbol declared dynamic with defdyn. Quote it: (dynamic? '*foo*)."),
    ("throw", &["x"], "Raise x as an error - a non-local exit caught by try/catch."),
    ("%make-macro", &["f"], "Tag fn f as a macro: the expander calls it on the unevaluated argument forms and splices its result in place. The `defmacro` macro lowers to this."),
    ("%spawn", &["thunk"], "Run thunk (a 0-arg fn) in a new green process; returns its pid. Use the `spawn` macro."),
    ("send", &["target", "msg"], "Copy msg into target's mailbox; target is a pid or {:name :node} address. Routes locally or over a node link. Returns nil."),
    ("self", &[], "This process's own pid (carries this node's identity)."),
    ("exit", &["pid", "reason"], "Send an exit signal to process pid, local or remote (Erlang exit/2). reason :kill is the untrappable hard kill — pid dies at its next reduction tick, or immediately if parked. Any other reason is the soft signal — pid dies at its next receive. Monitors fire [:down ref pid reason]. A remote pid is routed to its node over the link. No-op for a dead/unknown pid. Returns nil."),
    ("ref", &[], "A fresh, globally-unique reference token (tags a request to its reply)."),
    ("monitor", &["pid"], "Watch pid; returns a monitor ref. Delivers [:down ref pid reason] when pid dies."),
    ("list-processes", &[], "Every currently-live pid on this runtime (one per registered mailbox). Order is unspecified — sort if you need stability. For agents/tools enumerating spawned processes."),
    ("mailbox-size", &["pid"], "How many messages are queued in pid's mailbox (its receive backlog), or nil if pid is not a live local process. The one process-introspection accessor not reachable from Brood; see std/observer.blsp."),
    ("process-info", &["pid"], "A snapshot map of a live local process: {:id :pid :node :name :status :mailbox :monitored-by :parent :memory :collections :reductions} (:pid the process's pid value, for acting on it with exit/send/monitor; :status is :running or :waiting; :name nil if unregistered; :parent the spawner's id, nil for the root; :memory the LOCAL heap bytes and :collections the cumulative GC count, both as of the process's last receive; :reductions the cumulative reduction count — Erlang's scheduling unit, updated every quantum; exact for spawned processes, coarse for the root). nil for a remote/dead pid. The Erlang-process_info-style introspection the observer reads; see std/observer.blsp."),
    ("term-enter", &[], "Enter raw mode + the alternate screen, hide the cursor, and enable mouse capture, taking over the terminal for a full-screen UI (so click/scroll reach term-poll). Pair with term-leave. (ADR-046 display seam.)"),
    ("term-leave", &[], "Restore the terminal: show the cursor, disable mouse capture, leave the alternate screen, disable raw mode. The normal-path teardown for term-enter."),
    ("term-size", &[], "The terminal size as [cols rows] in character cells."),
    ("term-poll", &["ms"], "Wait up to ms milliseconds for an input event; return a key (a 1-char string for printables, or a keyword for specials: :up :down :left :right :enter :escape :backspace :tab :back-tab :delete :home :end :page-up :page-down, ctrl combos like :ctrl-c, alt combos like :alt-f), a mouse event as a vector [:mouse action button row col mods] (action: :press :release :drag :scroll-up :scroll-down — :drag is motion with a button held, reported once per cell crossed; button: :left :right :middle or nil for scroll; row/col 0-based cells; mods a vector of held modifier keywords in :ctrl :alt :shift order, [] when none — so Ctrl+wheel etc. are bindable), or nil on timeout. Always pass a finite ms."),
    ("term-draw", &["frame"], "Paint a frame — a vector of render ops: [:clear], [:text row col str], [:text row col str face], [:rect row col w h face], [:cursor row col] / [:cursor row col style]. A face is a map like {:fg :red :bold true}; a colour is a palette keyword (:red … :dark-grey, the terminal's named colour) or an explicit [r g b] vector / \"#rrggbb\" hex string (a true-colour cell). [:rect …] fills a w×h cell block with the face's background (a solid panel). The optional cursor `style` is :block (default), :bar, or :underline — the steady caret shape. The in-process frontend for the display protocol; returns nil."),
    ("gui-open", &["title?", "width?", "height?"], "Open a new native window and return its integer id (needs the runtime built with --features gui; errors otherwise). An optional `title` string sets the OS title-bar text (default `Brood`); change it later with gui-title!. Optional `width` `height` (logical pixels, both required together) set the initial window size (default 840x560). Its key/mouse input is delivered to the CALLING process's mailbox as messages — a key as a 1-char string / keyword (`:up`, `:ctrl-c`), the mouse as `[:mouse action button row col mods]` (action `:press`/`:release`/`:drag`/`:move`/`:scroll-up`/`:scroll-down` — `:drag` is motion with a button held and `:move` is bare motion with none (button nil), both delivered once per cell crossed (so mouse-look / hover need no click); `mods` a vector of held modifier keywords in `:ctrl :alt :shift` order, `[]` when none, so Ctrl+wheel / Ctrl+drag are bindable; a `:press` carries a trailing 7th element, its click-chain count `[… mods n]` — 1 single, 2 double, 3 triple, … for repeated presses of the same button in the same cell within the double-click window, so double-click-to-select-word and triple-click-to-select-line are bindable; the terminal reports 1), a resize as `[:resize cols rows]` (the new cell grid, so the loop re-renders at the new size) — so the consumer parks in `(receive)` instead of polling (ADR-058). Clicking the window's close button delivers a dedicated `:close` message — distinct from the Escape *key* (`:escape`), so an app can quit on the X without conflating it with Escape (which an editor binds to cancel/normal-mode); `ui-run` quits on `:close` automatically. Starts the GUI thread on the first call; each call is an independent window, so several observers can run at once. Pass the id to the other gui-* primitives; pair with gui-close."),
    ("gui-close", &["id"], "Close window id (the teardown for gui-open). Idempotent; an unknown id is a no-op."),
    ("gui-title!", &["id", "text"], "Set window id's OS title-bar text to the string text at runtime (the title gui-open gave it, or the default, otherwise). Needs --features gui; a no-op if the GUI thread never started or id isn't a live window. Returns nil."),
    ("gui-icon!", &["id", "rgba", "w", "h"], "Set window id's taskbar / title-bar icon from raw RGBA pixels: rgba is a vector of w*h*4 byte ints (0-255), row-major, 4 per pixel (red, green, blue, alpha). Needs --features gui; a silent no-op if the GUI thread never started, id isn't a live window, or the data length isn't w*h*4. Where the OS shows it depends on the platform (X11/Windows use it directly; Wayland prefers a .desktop file). Returns nil."),
    ("gui-focus", &["id"], "Raise window id to the front and give it OS keyboard focus, un-minimising it first. Lets an app surface an already-open (singleton) window instead of opening a duplicate — e.g. `(observe)` focuses its existing window rather than spawning a second. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-grab-cursor", &["id", "on"], "Confine the pointer to window id while `on` is truthy, release it otherwise — for mouse-look that shouldn't let the cursor slip out of the window and click another app. Uses the platform's `Confined` grab (cursor stays inside but keeps moving, so an absolute position-based look maps edge-to-edge), falling back to `Locked` where that's all the platform offers. Off by default; an app opts in. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-fullscreen!", &["id", "on"], "Make window id borderless-fullscreen while `on` is truthy (covering the whole monitor it's on, NO title bar / decorations — distraction-free), or restore it to a normal window otherwise. For a big-but-normal window that keeps its title bar, use gui-maximize! instead. The fullscreen/restore triggers a resize, so the consumer gets the usual [:resize cols rows] message and re-renders at the new size. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-maximize!", &["id", "on"], "Maximise window id while `on` is truthy (fill the screen's work area, KEEPING the title bar / decorations), or restore it to its previous size otherwise — e.g. an editor's init file opening big without going true-fullscreen. The maximise/restore triggers a resize, so the consumer gets the usual [:resize cols rows] message and re-renders at the new size. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-size", &["id"], "Window id's size as [cols rows] in character cells (tracks resize / HiDPI), same shape as term-size."),
    ("gui-held-key", &["id"], "The key window id currently sees as physically held — the same value its press delivered (a 1-char string, or a keyword like :ctrl-n / :up) — or nil when none is held. Tracked from press/release transitions in the event loop (NOT winit's ke.repeat, unreliable on Wayland), so it's the source of truth for a held key: a consumer-paced auto-repeat polls it each tick and stops the instant it no longer matches, so a missed key-up (e.g. lost on focus change) can't cause runaway repeat."),
    ("gui-draw", &["id", "frame"], "Paint a frame (the same render-op vector term-draw takes) to window id; returns nil. Unknown ops are skipped (forward-compatible). A text op's face may carry :scale n (GUI only, integer >=1, capped at 16): the text is drawn n× larger in an n×n block of cells anchored at its row/col — the per-pane/per-buffer font knob; the terminal frontend renders scale 1. A `[:cursor row col]` op may carry an optional `style` keyword (`[:cursor row col style]`) — :block (default, a 50% overlay), :bar (a thin caret on the cell's left edge), or :underline (a rule along the cell bottom). A `[:rect row col w h face]` op fills a w×h cell block with the face's background colour — a solid panel painted directly (no glyphs), the multi-row generalisation of a status bar. A `[:cursor-zone x y w h shape]` op marks a hover hot-zone: while the pointer is over it the window shows the resize cursor `shape` (:col-resize ↔ / :row-resize ↕), hit-tested on the GUI thread (ADR-080); it draws nothing and the terminal ignores it. A `[:vspans row0 col0 cols]` op is the column-renderer fast path (raycasters, spectrum bars): `cols` is a vector with one entry per cell-column (`col0`, `col0+1`, …), each a top-to-bottom stack of `[height colour]` segments painted from `row0` down — `colour` a face keyword (`:red`), an `[r g b]` triple (0..255), or nil (transparent). The per-cell fill happens natively here, so a wide scene costs the Brood side O(columns), not O(cells); GUI-only (the terminal ignores it)."),
    ("gui-font!", &["id?", "spec"], "Set a cell font from spec, a map {:family <keyword> :height <px>} (both keys optional): :family picks a registered font family (bundled :mono, or one added by gui-font-register), :height the cell pixel size. (gui-font! spec) sets the global default — every open window and ones opened later; (gui-font! id spec) retunes just window id, leaving the global default and other windows alone, so two windows can run different fonts. Per-section fonts within a window come from a face's :family/:scale. Needs --features gui. Returns nil."),
    ("gui-inset!", &["px"], "Set the window content inset to px logical pixels: a blank margin before the cell grid on every window edge, so a GUI app's text breathes instead of sitting flush against the frame. Applies to every open window and the default for ones opened later; the grid loses 2*px per axis (fewer cells) and re-renders. The inset is shared by the renderer and mouse hit-testing, so clicks stay aligned. Needs --features gui. Returns nil."),
    ("gui-font-register", &["name", "styles"], "Register font family name (a keyword) from styles, a map of style → TTF file path {:regular \"…\" :bold \"…\" :italic \"…\" :bold-italic \"…\"}. Only :regular is required; a missing style reuses the regular file. Afterwards a face's :family <name> (or gui-font!) selects it. Needs --features gui. Returns name."),
    ("term-raw-enter", &[], "Enter raw mode only — NO alternate screen, cursor stays visible, scrollback preserved. The seam for an inline line editor (the REPL); use term-enter instead for a full-screen TUI. Pair with term-raw-leave."),
    ("term-raw-leave", &[], "Leave raw mode (the teardown for term-raw-enter). Idempotent with the panic-path restore."),
    ("term-emit", &["ops"], "Paint inline, relative-motion render ops (for an in-place editor that must not take over the screen): [:print str], [:print str face], [:cr], [:nl], [:up n], [:down n], [:col n], [:clear-eol], [:clear-below], [:clear-screen]. A face is a map like {:fg :cyan :bold true}. Queues all ops then flushes once; unknown ops are skipped; returns nil."),
    ("demonitor", &["mref"], "Drop the monitor identified by mref (best-effort)."),
    ("link", &["pid"], "Symmetrically link the current process and pid, local or remote (Erlang link/1). When either dies, the other gets a [:EXIT pid reason] message if it set (trap-exit true), else dies too on an abnormal reason (propagation cascades through links; :normal does not propagate). A remote link fires :noconnection on net-split; linking an already-dead/unreachable pid notifies the caller (:noproc / :noconnection). Returns nil."),
    ("unlink", &["pid"], "Drop the symmetric link between the current process and pid (local or remote; best-effort). Returns nil."),
    ("trap-exit", &["on"], "Set the current process's trap_exit flag (Erlang process_flag(trap_exit, …)); returns the previous value. When on, a linked peer's death arrives as a trappable [:EXIT pid reason] message instead of killing this process."),
    ("spawn-count", &[], "How many green processes have been spawned since program start."),
    ("peak-threads", &[], "High-water mark of OS threads running processes concurrently."),
    ("worker-threads", &[], "The size of the scheduler's worker-thread pool (about nproc)."),
    ("steal-count", &[], "How many fresh processes the scheduler work-stole across worker threads since program start; 0 means placement-at-spawn kept the pool even."),
    ("register", &["name", "pid"], "Bind a local name so peers can address this process via {:name name :node this-node}. Returns the pid."),
    ("whereis", &["name"], "The local pid registered under `name`, or nil. Strictly local — does not query other nodes."),
    ("node-name", &[], "This runtime's node name (:nonode until node-start)."),
    ("nodes", &[], "A list of currently connected peer node names."),
    ("monitor-node", &["name"], "Get [:nodedown name] when the link to node `name` goes down (heartbeat timeout or close)."),
    ("disconnect", &["name"], "Tear down the link to peer node `name` now, without exiting this process (Erlang's disconnect_node) — fires [:nodedown name] on both sides and prunes `name` from (nodes). Returns true if a link existed, false otherwise. Use it to leave a node/cluster cleanly while staying alive."),
];

/// The `(params, doc)` for a primitive `name`, or `(&[], "")` if undocumented.
fn primitive_doc(name: &str) -> (&'static [&'static str], &'static str) {
    PRIMITIVE_DOCS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|&(_, p, d)| (p, d))
        .unwrap_or((&[], ""))
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Nil)
}

/// Destructure exactly two args. The declared `Arity` is the *primary* arity
/// check (enforced once in `eval::call_native` before any builtin runs); this
/// re-check is defense-in-depth for a direct Rust call that bypasses the gate
/// (e.g. a unit test) — it keeps such a call a clean error instead of a panic.
fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!(
            "{}: expected 2 arguments, got {}",
            who,
            args.len()
        )));
    }
    Ok((args[0], args[1]))
}

// ---------- numeric ----------

/// Require a value of a particular shape, or raise a self-identifying type
/// error attributed to `who` (the primitive that needed it). One macro behind
/// every `expect_*` helper below — the alternative was six hand-written
/// `match v { Value::X(id) => Ok(id), _ => Err(wrong_type(…, "kind", v)) }`
/// copies that drifted on the error helper used (`expect_node_name` chose
/// `type_err` over `wrong_type` and lost the offending value from its
/// message). The macro lifts that one rule into one place; the human-readable
/// `$expected` string is what the error message will say.
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

/// Require a number, coerced to `f64`; otherwise a self-identifying type error
/// attributed to `who` (the primitive that needed it).
fn expect_number(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    expect!(heap, who, v, "number",
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
    )
}

/// Require a string, returned **owned** so the `heap` borrow is released before
/// the builtin reads or allocates further (most callers go on to touch
/// `&mut heap`). The string analogue of [`expect_int`]/[`expect_number`].
fn expect_string(heap: &Heap, who: &str, v: Value) -> Result<String, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => heap.string(id).to_string(),
    )
}

/// Require a rope, returned **owned** (a cheap `Arc`-node clone) so the `heap`
/// borrow is released before the builtin edits or allocates a fresh rope.
fn expect_rope(heap: &Heap, who: &str, v: Value) -> Result<ropey::Rope, LispError> {
    expect!(heap, who, v, "rope",
        Value::Rope(id) => heap.rope(id).clone(),
    )
}

/// Require an integer; otherwise a self-identifying type error.
fn expect_int(heap: &Heap, who: &str, v: Value) -> Result<i64, LispError> {
    expect!(heap, who, v, "int",
        Value::Int(n) => n,
    )
}

/// Require an integer (`Int` or `BigInt`), coerced to `num_bigint::BigInt`;
/// otherwise the standard self-identifying type error (which prints the offending
/// value). The bignum analogue of [`expect_int`] — `expect_int` rejects a
/// `BigInt`, but the bitwise / bignum-aware ops accept either, so they route
/// through here instead of losing the value to a bare `type_err`.
fn expect_bigint(heap: &Heap, who: &str, v: Value) -> Result<num_bigint::BigInt, LispError> {
    heap.as_bigint(v)
        .ok_or_else(|| LispError::wrong_type(heap, who, "int", v))
}

/// Require a symbol; otherwise a self-identifying type error.
fn expect_symbol(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "symbol",
        Value::Sym(s) => s,
    )
}

/// True iff `v` is an integer (`Int` or `BigInt`) — the operand shape that
/// routes `+`/`-`/`*` through the bignum-promoting integer path rather than the
/// float path.
fn is_integer(v: Value) -> bool {
    matches!(v, Value::Int(_) | Value::BigInt(_))
}

/// Coerce an integer-or-float `Value` to `f64` for the float arithmetic path —
/// like [`expect_number`] but a `BigInt` also coerces (via its `to_f64`), so a
/// mixed `(+ 2^200 1.5)` works rather than rejecting the bignum.
fn num_to_f64(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    use num_traits::ToPrimitive;
    match v {
        Value::BigInt(id) => Ok(heap.bigint(id).to_f64().unwrap_or(f64::INFINITY)),
        _ => expect_number(heap, who, v),
    }
}

/// The kernel of `+`/`-`/`*`. Two `Int`s try `int_op` (a `checked_*`) first and
/// stay an `Int` on success; on overflow — or when either operand is already a
/// `BigInt` — both operands promote to `num_bigint::BigInt`, `big_op` computes,
/// and the result demotes through [`Heap::int_from_bigint`] (so it comes back as
/// an `Int` whenever it fits). A float operand keeps the old `f64` path.
fn num_bin(
    heap: &mut Heap,
    args: &[Value],
    who: &str,
    int_op: fn(i64, i64) -> Option<i64>,
    big_op: fn(num_bigint::BigInt, num_bigint::BigInt) -> num_bigint::BigInt,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let (a, b) = two(args, who)?;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => match int_op(x, y) {
            Some(r) => Ok(Value::Int(r)),
            // Overflowed i64 — redo in BigInt and demote (route through the
            // normalizer for one code path; here the result is out of range).
            None => {
                let r = big_op(num_bigint::BigInt::from(x), num_bigint::BigInt::from(y));
                Ok(heap.int_from_bigint(r))
            }
        },
        // At least one BigInt, both integers: promote both, compute, demote.
        _ if is_integer(a) && is_integer(b) => {
            let x = heap.as_bigint(a).expect("integer");
            let y = heap.as_bigint(b).expect("integer");
            let r = big_op(x, y);
            Ok(heap.int_from_bigint(r))
        }
        // A float operand anywhere: the float path (a BigInt coerces via `f64`).
        _ => Ok(Value::Float(float_op(
            num_to_f64(heap, who, a)?,
            num_to_f64(heap, who, b)?,
        ))),
    }
}

fn prim_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(
        heap,
        args,
        "%add",
        i64::checked_add,
        |a, b| a + b,
        |a, b| a + b,
    )
}
fn prim_sub(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(
        heap,
        args,
        "%sub",
        i64::checked_sub,
        |a, b| a - b,
        |a, b| a - b,
    )
}
fn prim_mul(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(
        heap,
        args,
        "%mul",
        i64::checked_mul,
        |a, b| a * b,
        |a, b| a * b,
    )
}

fn prim_div(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%div")?;
    let bf = num_to_f64(heap, "%div", b)?;
    if bf == 0.0 {
        return Err(LispError::runtime("division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (/ x y))"));
    }
    match (a, b) {
        // Exact integer quotient when it divides evenly; otherwise a float.
        // `checked_*` guards the one overflowing case (`i64::MIN / -1`), which
        // then falls through to the float path instead of panicking.
        (Value::Int(x), Value::Int(y)) => match (x.checked_rem(y), x.checked_div(y)) {
            (Some(0), Some(q)) => Ok(Value::Int(q)),
            _ => Ok(Value::Float(x as f64 / y as f64)),
        },
        // Both integers, at least one a BigInt: exact quotient when it divides
        // evenly (demoted — `(/ 2^200 2^100)` is the exact `2^100`); otherwise a
        // float. Division by zero was already caught via `bf == 0.0`.
        _ if is_integer(a) && is_integer(b) => {
            use num_integer::Integer;
            let x = heap.as_bigint(a).expect("integer");
            let y = heap.as_bigint(b).expect("integer");
            let (q, r) = x.div_rem(&y);
            if num_traits::Zero::is_zero(&r) {
                Ok(heap.int_from_bigint(q))
            } else {
                Ok(Value::Float(num_to_f64(heap, "%div", a)? / bf))
            }
        }
        _ => Ok(Value::Float(num_to_f64(heap, "%div", a)? / bf)),
    }
}

fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%lt")?;
    // Compare two integers directly; coercing to f64 first loses precision past
    // 2^53 (e.g. `(< 9007199254740992 9007199254740993)` would wrongly be false).
    // `value_cmp` already handles Int/BigInt exactly and the mixed int/float and
    // BigInt/float cases.
    let lt = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x < y,
        _ if is_integer(a) && is_integer(b) => heap.value_cmp(a, b) == std::cmp::Ordering::Less,
        _ => num_to_f64(heap, "%lt", a)? < num_to_f64(heap, "%lt", b)?,
    };
    Ok(Value::Bool(lt))
}

/// `(%le a b)` — `a <= b`. The `<=`/`>=` kernel: a direct primitive so the 2-arg
/// clauses of `<=`/`>=` are pure passthroughs the ADR-069 thin-wrapper elision can
/// reach (the old `(not (%lt …))` bodies were a nested call it couldn't). Same
/// int-exact / float-coerce care as `%lt`.
fn prim_le(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%le")?;
    let le = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x <= y,
        _ if is_integer(a) && is_integer(b) => heap.value_cmp(a, b) != std::cmp::Ordering::Greater,
        _ => num_to_f64(heap, "%le", a)? <= num_to_f64(heap, "%le", b)?,
    };
    Ok(Value::Bool(le))
}

fn prim_eq(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, kw::EQ_PRIM)?;
    Ok(Value::Bool(heap.equal(a, b)))
}

/// Read two arguments as `num_bigint::BigInt`s (`Int`s promote), for the
/// bignum-aware integer ops (`rem`/`quot`/the bitwise family). A self-identifying
/// type error if either isn't an integer.
fn bigint_pair(
    heap: &Heap,
    args: &[Value],
    who: &str,
) -> Result<(num_bigint::BigInt, num_bigint::BigInt), LispError> {
    let (a, b) = two(args, who)?;
    let x = expect_bigint(heap, who, a)?;
    let y = expect_bigint(heap, who, b)?;
    Ok((x, y))
}

fn remainder(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "rem")?;
    // i64 fast path. `checked_rem` returns None on `b == 0` (div-by-zero) and
    // on the lone `i64::MIN % -1` overflow — that overflow is mathematically 0,
    // so handle it directly rather than promoting.
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        return match x.checked_rem(y) {
            Some(r) => Ok(Value::Int(r)),
            None if y == 0 => Err(LispError::runtime("rem: division by zero")
                .with_code(crate::error::error_codes::DIV_BY_ZERO)
                .with_hint("guard the denominator: (when (not= y 0) (rem x y))")),
            None => Ok(Value::Int(0)), // i64::MIN % -1
        };
    }
    let (x, y) = bigint_pair(heap, args, "rem")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("rem: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (rem x y))"));
    }
    // `BigInt::%` truncates toward zero (matches i64 `%`), so the remainder has
    // the dividend's sign — the non-Euclidean `rem` the prelude `mod` builds on.
    Ok(heap.int_from_bigint(x % y))
}

/// `(%quot a b)` — truncating integer division toward zero, the kernel `quot`
/// passes through to. `checked_div` truncates toward zero (matching the old
/// `(/ (- a (rem a b)) b)` integer result) and guards both `b == 0` and the lone
/// `i64::MIN / -1` overflow; that overflow promotes to BigInt.
fn prim_quot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%quot")?;
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        match x.checked_div(y) {
            Some(q) => return Ok(Value::Int(q)),
            None if y == 0 => {
                return Err(LispError::runtime("quot: division by zero")
                    .with_code(crate::error::error_codes::DIV_BY_ZERO)
                    .with_hint("guard the denominator: (when (not= y 0) (quot x y))"))
            }
            None => {} // i64::MIN / -1 — promote and fall through
        }
    }
    let (x, y) = bigint_pair(heap, args, "%quot")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("quot: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (quot x y))"));
    }
    // `BigInt::/` truncates toward zero, matching i64 `checked_div`.
    Ok(heap.int_from_bigint(x / y))
}

/// Floor toward negative infinity, returning an `Int` — the one Float→Int
/// crossing the language can't bootstrap (no other primitive produces an `Int`
/// from a `Float`). An `Int` passes through; a `Float` is floored. `NaN` and
/// values whose floor doesn't fit in `i64` are runtime errors — pre-fix the
/// `as i64` cast silently saturated, so `(floor (* 1e308 1e308))` returned
/// `i64::MAX` and `(floor (/ 0.0 0.0))` returned `0`. `ceil`/`round`/`quot`/
/// `pow`/`sqrt` are all Brood over this + `rem`/`/`/`*`/`<` (std/prelude.blsp).
fn floor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::Int(n)),
        // A bignum is already an integer — it is its own floor.
        v @ Value::BigInt(_) => Ok(v),
        v => {
            let f = expect_number(heap, "floor", v)?.floor();
            if !f.is_finite() {
                return Err(LispError::runtime(format!(
                    "floor: argument {} has no integer floor",
                    f
                ))
                .with_code(crate::error::error_codes::INT_OVERFLOW));
            }
            // `i64::MIN as f64` rounds *down* to a value still in range; the
            // upper bound `i64::MAX as f64` rounds *up* past `i64::MAX`, so
            // the open upper comparison is the right one.
            if f < i64::MIN as f64 || f >= i64::MAX as f64 + 1.0 {
                return Err(
                    LispError::runtime(format!("floor: {} is out of range for i64", f))
                        .with_code(crate::error::error_codes::INT_OVERFLOW),
                );
            }
            Ok(Value::Int(f as i64))
        }
    }
}

// ---------- bitwise ----------

fn bit_and(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::Int(a & b));
    }
    // num-bigint implements bitwise ops on its (infinite) two's-complement
    // model, so this matches the i64 result on small values and extends it.
    let (a, b) = bigint_pair(heap, args, "bit-and")?;
    Ok(heap.int_from_bigint(a & b))
}

fn bit_or(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::Int(a | b));
    }
    let (a, b) = bigint_pair(heap, args, "bit-or")?;
    Ok(heap.int_from_bigint(a | b))
}

fn bit_xor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::Int(a ^ b));
    }
    let (a, b) = bigint_pair(heap, args, "bit-xor")?;
    Ok(heap.int_from_bigint(a ^ b))
}

fn bit_not(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::Int(!n)),
        Value::BigInt(id) => {
            let n = !heap.bigint(id).clone();
            Ok(heap.int_from_bigint(n))
        }
        v => Err(LispError::wrong_type(heap, "bit-not", "int", v)),
    }
}

fn bit_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::Int(i64::from(n.count_ones()))),
        // Popcount of the MAGNITUDE (abs value) — the bitboard only uses
        // non-negative values, so we count the set bits of |n| (`BigUint`'s
        // `count_ones`), sign-independent.
        Value::BigInt(id) => {
            let bits = heap.bigint(id).magnitude().count_ones();
            Ok(Value::Int(bits as i64))
        }
        v => Err(LispError::wrong_type(heap, "bit-count", "int", v)),
    }
}

fn bit_positions(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // The 0-based indices of the set bits, ascending. O(popcount): pull the
    // lowest set bit, record it, clear it, repeat — so enumerating a sparse
    // bitset costs the number of members, not the bit width (the bitboard
    // renderer leans on this to stay O(live) instead of O(area)).
    let mut out: Vec<Value> = Vec::new();
    match arg(args, 0) {
        Value::Int(n) => {
            let mut bits = n as u64; // the two's-complement bit pattern (bitboard words are non-negative)
            while bits != 0 {
                out.push(Value::Int(i64::from(bits.trailing_zeros())));
                bits &= bits - 1; // clear the lowest set bit
            }
        }
        Value::BigInt(id) => {
            let mut mag = heap.bigint(id).magnitude().clone();
            while let Some(i) = mag.trailing_zeros() {
                out.push(Value::Int(i as i64));
                mag.set_bit(i, false);
            }
        }
        v => return Err(LispError::wrong_type(heap, "bit-positions", "int", v)),
    }
    Ok(heap.alloc_vector(out))
}

// ===== bitset: refc-shared fixed-size bit array (POC) ============================
// A bitset is read WITHOUT copying its bytes: a shared one hands back its `Arc`
// (a refcount bump), so the bitwise ops touch the blob in place and only the
// RESULT is allocated. (An inline string — only happens off the bitset path —
// falls back to a one-time copy.)
enum BsData {
    Shared(std::sync::Arc<crate::core::blob::SharedBlob>),
    Owned(Vec<u8>),
}
impl BsData {
    fn bytes(&self) -> &[u8] {
        match self {
            BsData::Shared(a) => a.as_bytes(),
            BsData::Owned(v) => v,
        }
    }
}

fn bs_arc(heap: &Heap, v: Value, who: &str) -> Result<BsData, LispError> {
    match v {
        Value::Str(id) => Ok(match heap.local_shared_blob(id) {
            Some(a) => BsData::Shared(a),
            None => BsData::Owned(heap.string(id).as_bytes().to_vec()),
        }),
        _ => Err(LispError::wrong_type(heap, who, "bitset", v)),
    }
}

// Allocate a bitset from raw bytes — ALWAYS shared, so send/table ship it by reference.
fn bs_alloc(heap: &mut Heap, bytes: &[u8]) -> Value {
    heap.alloc_string_from_shared(crate::core::blob::SharedBlob::new(bytes))
}

fn bs_nbytes(nbits: i64) -> usize {
    ((nbits.max(0) as usize) + 7) / 8
}

fn bs_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "bitset", arg(args, 0))?;
    Ok(bs_alloc(heap, &vec![0u8; bs_nbytes(n)]))
}

fn bs_ones(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "bitset-ones", arg(args, 0))?.max(0) as usize;
    let len = (n + 7) / 8;
    let mut out = vec![0xffu8; len];
    if len > 0 && n % 8 != 0 {
        out[len - 1] = (1u8 << (n % 8)) - 1;
    }
    Ok(bs_alloc(heap, &out))
}

fn bs_binop(heap: &mut Heap, args: &[Value], who: &str, f: fn(u8, u8) -> u8) -> LispResult {
    let a = bs_arc(heap, arg(args, 0), who)?;
    let b = bs_arc(heap, arg(args, 1), who)?;
    let (ab, bb) = (a.bytes(), b.bytes());
    let out: Vec<u8> = (0..ab.len()).map(|i| f(ab[i], *bb.get(i).unwrap_or(&0))).collect();
    Ok(bs_alloc(heap, &out))
}

// Fused bit-plane full-adder: sum a vector of bitsets bit-by-bit into the low THREE
// planes [s0 s1 s2] of the per-bit count, in ONE native pass (no per-op allocation —
// the ~40-op interpreted reduce this replaces alloc'd a fresh bitset per step). The
// adder is bitwise-parallel: each byte carries 8 independent bit-columns; the carry
// flows between PLANES (s0→s1→s2), not between bits. General over any field count.
fn bs_planes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = match arg(args, 0) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        v => return Err(LispError::wrong_type(heap, "bitset-planes", "vector", v)),
    };
    let datas: Vec<BsData> = items
        .iter()
        .map(|x| bs_arc(heap, *x, "bitset-planes"))
        .collect::<Result<_, _>>()?;
    let len = datas.first().map_or(0, |d| d.bytes().len());
    let (mut s0, mut s1, mut s2) = (vec![0u8; len], vec![0u8; len], vec![0u8; len]);
    for d in &datas {
        let m = d.bytes();
        for j in 0..len {
            let mj = *m.get(j).unwrap_or(&0);
            let c = s0[j] & mj;
            s0[j] ^= mj;
            let c2 = s1[j] & c;
            s1[j] ^= c;
            s2[j] ^= c2;
        }
    }
    let r0 = bs_alloc(heap, &s0);
    let r1 = bs_alloc(heap, &s1);
    let r2 = bs_alloc(heap, &s2);
    Ok(heap.alloc_vector(vec![r0, r1, r2]))
}

fn bs_and(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    bs_binop(heap, args, "bitset-and", |x, y| x & y)
}
fn bs_or(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    bs_binop(heap, args, "bitset-or", |x, y| x | y)
}
fn bs_xor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    bs_binop(heap, args, "bitset-xor", |x, y| x ^ y)
}

fn bs_shl(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-shl")?;
    let src = srcd.bytes();
    let n = expect_int(heap, "bitset-shl", arg(args, 1))?.max(0) as usize;
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in bo..len {
        let lo = src[j - bo] as u16;
        let mut v = lo << bs;
        if bs != 0 && j - bo >= 1 {
            v |= (src[j - bo - 1] as u16) >> (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    Ok(bs_alloc(heap, &out))
}

fn bs_shr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-shr")?;
    let src = srcd.bytes();
    let n = expect_int(heap, "bitset-shr", arg(args, 1))?.max(0) as usize;
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in 0..len {
        let hi = j + bo;
        if hi >= len {
            break;
        }
        let mut v = (src[hi] as u16) >> bs;
        if bs != 0 && hi + 1 < len {
            v |= (src[hi + 1] as u16) << (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    Ok(bs_alloc(heap, &out))
}

fn bs_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut src = bs_arc(heap, arg(args, 0), "bitset-set")?.bytes().to_vec();
    let i = expect_int(heap, "bitset-set", arg(args, 1))?.max(0) as usize;
    let (byte, bit) = (i / 8, i % 8);
    if byte < src.len() {
        src[byte] |= 1 << bit;
    }
    Ok(bs_alloc(heap, &src))
}

fn bs_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-count")?;
    let n: u32 = srcd.bytes().iter().map(|b| b.count_ones()).sum();
    Ok(Value::Int(n as i64))
}

fn bs_positions(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-positions")?;
    let src = srcd.bytes();
    let mut out: Vec<Value> = Vec::new();
    for (bi, &byte) in src.iter().enumerate() {
        let mut b = byte;
        while b != 0 {
            out.push(Value::Int((bi * 8 + b.trailing_zeros() as usize) as i64));
            b &= b - 1;
        }
    }
    Ok(heap.alloc_vector(out))
}

// Pure byte-buffer bit ops (LSB-first, fixed length) used by the fused step below.
fn b_shl(src: &[u8], n: usize) -> Vec<u8> {
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in bo..len {
        let mut v = (src[j - bo] as u16) << bs;
        if bs != 0 && j - bo >= 1 {
            v |= (src[j - bo - 1] as u16) >> (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    out
}
fn b_shr(src: &[u8], n: usize) -> Vec<u8> {
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in 0..len {
        let hi = j + bo;
        if hi >= len {
            break;
        }
        let mut v = (src[hi] as u16) >> bs;
        if bs != 0 && hi + 1 < len {
            v |= (src[hi + 1] as u16) << (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    out
}
fn b_and(a: &[u8], b: &[u8]) -> Vec<u8> {
    (0..a.len()).map(|i| a[i] & b.get(i).copied().unwrap_or(0)).collect()
}
fn b_or(a: &[u8], b: &[u8]) -> Vec<u8> {
    (0..a.len()).map(|i| a[i] | b.get(i).copied().unwrap_or(0)).collect()
}
fn b_xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    (0..a.len()).map(|i| a[i] ^ b.get(i).copied().unwrap_or(0)).collect()
}

// The whole Moore-8 torus neighbour sum in ONE native pass: builds the eight
// torus-shifted neighbour fields (west/east with column wrap, then each lifted a
// row up/down with row wrap) and full-adders them into the low 3 count planes
// [s0 s1 s2]. A general life-like-CA primitive; the survival RULE stays in Brood.
// Args: board bits, the precomputed col0/high/mask/board-mask bitsets, w, h.
fn bs_neighbour_sum(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "bitset-neighbour-sum";
    let b = bs_arc(heap, arg(args, 0), who)?;
    let col0 = bs_arc(heap, arg(args, 1), who)?;
    let high = bs_arc(heap, arg(args, 2), who)?;
    let mask = bs_arc(heap, arg(args, 3), who)?;
    let board = bs_arc(heap, arg(args, 4), who)?;
    let w = expect_int(heap, who, arg(args, 5))?.max(1) as usize;
    let h = expect_int(heap, who, arg(args, 6))?.max(1) as usize;
    let (bb, c0, hi, mk, bd) = (b.bytes(), col0.bytes(), high.bytes(), mask.bytes(), board.bytes());
    let (wm1, hm1w) = (w - 1, (h - 1) * w);

    // west / east fields (torus-wrapped within each row)
    let l = b_or(&b_and(&b_shl(bb, 1), &b_xor(c0, bd)), &b_shr(&b_and(bb, hi), wm1));
    let r = b_or(&b_and(&b_shr(bb, 1), &b_xor(hi, bd)), &b_shl(&b_and(bb, c0), wm1));
    let up = |f: &[u8]| b_or(&b_and(&b_shl(f, w), bd), &b_shr(f, hm1w));
    let dn = |f: &[u8]| b_or(&b_shr(f, w), &b_shl(&b_and(f, mk), hm1w));
    let ns: [Vec<u8>; 8] = [up(&l), up(bb), up(&r), l.clone(), r.clone(), dn(&l), dn(bb), dn(&r)];

    let len = bb.len();
    let (mut s0, mut s1, mut s2) = (vec![0u8; len], vec![0u8; len], vec![0u8; len]);
    for m in &ns {
        for j in 0..len {
            let mj = m[j];
            let c = s0[j] & mj;
            s0[j] ^= mj;
            let c2 = s1[j] & c;
            s1[j] ^= c;
            s2[j] ^= c2;
        }
    }
    let r0 = bs_alloc(heap, &s0);
    let r1 = bs_alloc(heap, &s1);
    let r2 = bs_alloc(heap, &s2);
    Ok(heap.alloc_vector(vec![r0, r1, r2]))
}

// A whole Conway step in ONE native op: the Moore-8 torus neighbour sum (above)
// plus the B3/S23 survival rule `s1 & ~s2 & (s0 | cur)`, returning the next board
// bitset directly — no intermediate Brood allocation. (Bakes the Life rule, unlike
// the general `bitset-neighbour-sum`.) Args as `bitset-neighbour-sum`.
fn bs_life_step(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "bitset-life-step";
    let b = bs_arc(heap, arg(args, 0), who)?;
    let col0 = bs_arc(heap, arg(args, 1), who)?;
    let high = bs_arc(heap, arg(args, 2), who)?;
    let mask = bs_arc(heap, arg(args, 3), who)?;
    let board = bs_arc(heap, arg(args, 4), who)?;
    let w = expect_int(heap, who, arg(args, 5))?.max(1) as usize;
    let h = expect_int(heap, who, arg(args, 6))?.max(1) as usize;
    let (bb, c0, hi, mk, bd) = (b.bytes(), col0.bytes(), high.bytes(), mask.bytes(), board.bytes());
    let (wm1, hm1w) = (w - 1, (h - 1) * w);

    let l = b_or(&b_and(&b_shl(bb, 1), &b_xor(c0, bd)), &b_shr(&b_and(bb, hi), wm1));
    let r = b_or(&b_and(&b_shr(bb, 1), &b_xor(hi, bd)), &b_shl(&b_and(bb, c0), wm1));
    let up = |f: &[u8]| b_or(&b_and(&b_shl(f, w), bd), &b_shr(f, hm1w));
    let dn = |f: &[u8]| b_or(&b_shr(f, w), &b_shl(&b_and(f, mk), hm1w));
    let ns: [Vec<u8>; 8] = [up(&l), up(bb), up(&r), l.clone(), r.clone(), dn(&l), dn(bb), dn(&r)];

    let len = bb.len();
    let mut next = vec![0u8; len];
    for j in 0..len {
        let (mut s0, mut s1, mut s2) = (0u8, 0u8, 0u8);
        for m in &ns {
            let mj = m[j];
            let c = s0 & mj;
            s0 ^= mj;
            let c2 = s1 & c;
            s1 ^= c;
            s2 ^= c2;
        }
        // survive iff (2 neighbours and alive) or 3 neighbours: s1 & ~s2 & (s0 | cur)
        next[j] = s1 & (s2 ^ bd[j]) & (s0 | bb[j]);
    }
    Ok(bs_alloc(heap, &next))
}

/// Validate a shift amount: non-negative (a negative shift is an error) and not
/// absurdly large (cap well above any realistic bit width so a typo'd
/// `(bit-shift-left 1 1e9)` can't try to allocate gigabytes). Returns the amount
/// as `usize`. No upper *bit-width* cap any more — large shifts promote to
/// BigInt (the whole point of the bitboard use).
fn shift_amount(n: i64, who: &str) -> Result<usize, LispError> {
    if n < 0 {
        return Err(LispError::runtime(format!(
            "{}: negative shift amount {}",
            who, n
        )));
    }
    // ~128 Mbit: far past any legitimate use, but bounds the worst-case alloc.
    const MAX_SHIFT: i64 = 1 << 27;
    if n > MAX_SHIFT {
        return Err(LispError::runtime(format!(
            "{}: shift amount {} too large (max {})",
            who, n, MAX_SHIFT
        )));
    }
    Ok(n as usize)
}

fn bit_shift_left(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let a = arg(args, 0);
    let n = expect_int(heap, "bit-shift-left", arg(args, 1))?;
    let amount = shift_amount(n, "bit-shift-left")?;
    // i64 fast path: stay an `Int` when the shift fits, else promote. (Unlike the
    // old wrapping shift, an i64 result that would lose bits past the top now
    // promotes to BigInt — the conventional arbitrary-width left shift.)
    if let Value::Int(x) = a {
        if amount < 64 {
            if let Some(r) = x.checked_shl(amount as u32) {
                // checked_shl only guards the *shift amount*, not value overflow;
                // verify the shift is lossless before keeping the i64 result.
                if (r >> amount) == x {
                    return Ok(Value::Int(r));
                }
            }
        }
    }
    let x = expect_bigint(heap, "bit-shift-left", a)?;
    Ok(heap.int_from_bigint(x << amount))
}

fn bit_shift_right(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let a = arg(args, 0);
    let n = expect_int(heap, "bit-shift-right", arg(args, 1))?;
    let amount = shift_amount(n, "bit-shift-right")?;
    // Arithmetic (sign-preserving) right shift, matching the signed model.
    if let Value::Int(x) = a {
        // A right shift ≥ 64 collapses to the sign bit (0 or -1).
        let r = if amount >= 64 { x >> 63 } else { x >> amount };
        return Ok(Value::Int(r));
    }
    let x = expect_bigint(heap, "bit-shift-right", a)?;
    Ok(heap.int_from_bigint(x >> amount))
}

// ---------- pair / sequence ----------

fn cons(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(heap.alloc_pair(a, b))
}

fn first(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.car(p)),
        Value::Vector(id) => Ok(heap.vector(id).first().copied().unwrap_or(Value::Nil)),
        // A range is non-empty by construction, so its head is `lo`.
        Value::Range(id) => Ok(Value::Int(heap.range_parts(id).0)),
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::wrong_type(heap, "first", "list or vector", v)),
    }
}

fn rest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
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
            Ok(heap.alloc_range(lo + step, hi, step))
        }
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::wrong_type(heap, "rest", "list or vector", v)),
    }
}

/// `(%range lo hi step)` — construct a lazy integer range. Returns `Nil` for an
/// empty range; errors on a zero step. The arg-parsing arities live in the
/// Brood `range`, which calls this with all three resolved.
fn range_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
fn range_pred(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Range(_))))
}

/// `(%range-count rng)` — the element count of a range, O(1).
fn range_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => Ok(Value::Int(heap.range_len(id))),
        Value::Nil => Ok(Value::Int(0)),
        v => Err(LispError::wrong_type(heap, "%range-count", "range", v)),
    }
}

/// `(%range->list rng)` — realise a range to a concrete list (the slow path
/// behind `seq`/`reverse`/`nth` on a range).
fn range_to_list(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Range(id) => {
            let items = heap.range_to_vec(id);
            Ok(heap.list(items))
        }
        Value::Nil => Ok(Value::Nil),
        v => Err(LispError::wrong_type(heap, "%range->list", "range", v)),
    }
}

/// `(%range-reduce f acc rng)` — left-fold a range with `f` in a native counted
/// loop, **without materialising** it: the whole point of the reducible range.
/// `acc` and `f` are rooted across the loop because each `apply` is a safepoint
/// that can relocate them.
fn range_reduce(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let f = arg(args, 0);
    let init = arg(args, 1);
    let (lo, hi, step) = match arg(args, 2) {
        Value::Range(id) => heap.range_parts(id),
        Value::Nil => return Ok(init), // empty range — acc unchanged
        v => return Err(LispError::wrong_type(heap, "%range-reduce", "range", v)),
    };
    // Route the per-element callback through the VM when it's the active engine
    // (`apply_value` → `dispatch`: a VM-eligible reducer runs compiled, ~the
    // big win for a named/RUNTIME reducer; a native or ineligible closure still
    // falls back to `eval::apply` inside `dispatch`). `BROOD_VM=0` keeps the pure
    // tree-walker path, so the escape hatch / differential TW mode stay honest.
    // Checked once, not per element.
    let use_vm = crate::eval::compile::vm_enabled();
    // Primitive-reducer fast path: when `f` is `+`/`*` (directly, or via the
    // prelude wrapper's passthrough arm), fold with the inlined scalar op and
    // never call back into `apply` per element — that per-element dispatch is the
    // dominant cost of `(reduce + 0 (range n))`. Resolved once against the live
    // env; a per-element miss (i64 overflow → BigInt, or a Float/BigInt acc) falls
    // back to the real reducer for that step, so the result stays bit-identical.
    let prim = crate::eval::compile::reduce_prim_op(heap, f);
    heap.root_scope(|heap| {
        let f_r = heap.root(f);
        let mut acc_r = heap.root(init);
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            let f = heap.read_root(f_r);
            let acc = heap.read_root(acc_r);
            let next = match prim {
                Some(op) => match crate::eval::compile::prim_apply_step(op, acc, Value::Int(i))? {
                    Some(v) => v,
                    None if use_vm => {
                        crate::eval::compile::apply_value(heap, f, &[acc, Value::Int(i)], env)?
                    }
                    None => apply(heap, f, &[acc, Value::Int(i)], env)?,
                },
                None if use_vm => {
                    crate::eval::compile::apply_value(heap, f, &[acc, Value::Int(i)], env)?
                }
                None => apply(heap, f, &[acc, Value::Int(i)], env)?,
            };
            acc_r = heap.advance_root(acc_r, next);
            i += step;
        }
        Ok(heap.read_root(acc_r))
    })
}

/// `(%sort-asc coll)` — stable ascending sort of a numeric collection by `<`.
/// The fast path behind `(sort coll)` when no custom comparator is given;
/// the all-Brood `merge-sort` in `std/prelude.blsp` still handles
/// `(sort less? coll)`. ~50× faster than the in-Brood mergesort on 10 000
/// items because every comparison is a Rust `match` instead of an
/// `eval::apply` round-trip.
///
/// Items must be `Int` / `Float` / mixed (the same shape `<` accepts).
/// Mixed Int+Float promote to float for the compare (matching `prim_lt`).
/// Any non-numeric item is a `wrong_type` error against the offending value.
fn sort_asc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Collect into a Vec. `seq_items` walks the cons spine (or copies a
    // vector) once. Values are `Copy` so the Vec holds plain handles — no
    // GC root machinery needed because `sort_by` does no eval and can't
    // trigger a safepoint.
    let mut items = heap.seq_items(arg(args, 0))?;

    // Validate before sorting so a non-numeric item produces one clear
    // error rather than an indeterminate-order partial sort.
    for &v in &items {
        match v {
            Value::Int(_) | Value::Float(_) => {}
            _ => return Err(LispError::wrong_type(heap, "sort", "number", v)),
        }
    }

    // Stable sort. The int-int branch keeps full precision; mixed pairs
    // promote to f64 (same compromise as `prim_lt`'s mixed case — past
    // 2^53 the float compare can collapse two distinct ints, but that
    // matches what `<` itself would do).
    items.sort_by(|a, b| match (*a, *b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(&y),
        _ => {
            let xf = match *a {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                _ => unreachable!(),
            };
            let yf = match *b {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                _ => unreachable!(),
            };
            // NaN sorts as Equal (would otherwise break `sort_by`'s total
            // ordering). Real Brood `<` doesn't admit NaN past `(nan? x)`
            // anyway, so this is the lesser evil.
            xf.partial_cmp(&yf).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    Ok(heap.list(items))
}

/// `(%sort-cmp coll)` — stable ascending sort by the structural total order
/// (`Heap::value_cmp`). The Brood `sort` (prelude) routes here when items
/// aren't all numeric, so `(sort [[1 0] [2 1]])` and similar work without a
/// custom comparator. Cross-kind items get a defined tag-rank order rather
/// than the old "expected number" trap.
fn sort_cmp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
fn compare(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::cmp::Ordering;
    let ord = match heap.value_cmp(arg(args, 0), arg(args, 1)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    Ok(Value::Int(ord))
}

// ---------- vector ----------

fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector(args.to_vec()))
}

fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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

fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Vector(id) => Ok(Value::Int(heap.vector(id).len() as i64)),
        _ => Err(LispError::wrong_type(heap, "vector-length", "vector", v)),
    }
}

/// `(vector-assoc v i x)` — a fresh vector like `v` with index `i` set to `x`.
/// The vector counterpart of `map-assoc`; O(n) copy (vectors are flat), one
/// allocation, no cons churn. `i` must be in `[0, len)` (append-at-end is a
/// deferred power feature, ADR-011). No GC safepoint runs inside a builtin, so
/// the cloned handles stay valid across `alloc_vector`.
fn vector_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
fn subvec(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
fn expect_map(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    expect!(heap, who, v, "map",
        Value::Map(id) => id,
    )
}

/// `(hash-map k v k v …)` — build a map from alternating key/value args (the
/// programmatic form of the `{ }` literal). Errors on an odd count; last-wins on
/// duplicate keys.
fn hash_map(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if !args.len().is_multiple_of(2) {
        return Err(LispError::arity(
            "hash-map: expected an even number of arguments (key/value pairs)",
        ));
    }
    let pairs: Vec<(Value, Value)> = args.chunks_exact(2).map(|kv| (kv[0], kv[1])).collect();
    Ok(heap.map_from_pairs(pairs))
}

/// The `[k v]` of a pair item — a `[k v]` vector or a `(k v)` list — with
/// `first`/`second` semantics (missing slots read as `nil`). Used by
/// [`map_into`] to read the items of an `into`/`zipmap` sequence.
fn pair_kv(heap: &Heap, who: &str, p: Value) -> Result<(Value, Value), LispError> {
    match p {
        Value::Vector(id) => {
            let v = heap.vector(id);
            Ok((
                v.first().copied().unwrap_or(Value::Nil),
                v.get(1).copied().unwrap_or(Value::Nil),
            ))
        }
        Value::Pair(id) => {
            let (k, rest) = heap.pair(id);
            let val = match rest {
                Value::Pair(rid) => heap.pair(rid).0,
                _ => Value::Nil,
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
fn map_into(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let into = expect_map(heap, "%map-into", arg(args, 0))?;
    let items = heap.seq_items(arg(args, 1))?;
    let mut pairs = Vec::with_capacity(items.len());
    for it in items {
        pairs.push(pair_kv(heap, "%map-into", it)?);
    }
    Ok(heap.map_from_pairs_into(into, pairs))
}

/// `(map-get m k [default])` — the value `k` maps to, or `default` (nil if
/// omitted) when absent.
fn map_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-get", arg(args, 0))?;
    Ok(heap
        .map_get(id, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(map-assoc m k v)` — a fresh map with `k` bound to `v`.
fn map_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-assoc", arg(args, 0))?;
    Ok(heap.map_assoc(id, arg(args, 1), arg(args, 2)))
}

/// `(map-dissoc m k)` — a fresh map with `k` removed.
fn map_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-dissoc", arg(args, 0))?;
    Ok(heap.map_dissoc(id, arg(args, 1)))
}

/// `(map-pairs m)` — the entries as a list of `[k v]` vectors, in insertion
/// order, in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`reduce-kv` are all Brood over it (std/prelude.blsp).
fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-pairs", arg(args, 0))?;
    let entries = heap.map_entries(id); // copy out, releasing the borrow before we alloc
    let pairs: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| heap.alloc_vector(vec![k, v]))
        .collect();
    Ok(heap.list(pairs))
}

/// `(map-count m)` — the number of entries, O(1). The CHAMP root node tracks
/// its subtree size, so this never walks (or allocates) the entries; it's what
/// `count`/`empty?` on a map use instead of materialising `map-pairs`.
fn map_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-count", arg(args, 0))?;
    Ok(Value::Int(heap.map_size(id) as i64))
}

// ---------- transient maps ----------

/// Pull a `TransientId` out of `v` or raise a self-identifying type error.
fn expect_transient(heap: &Heap, who: &str, v: Value) -> Result<value::TransientId, LispError> {
    expect!(heap, who, v, "transient",
        Value::Transient(id) => id,
    )
}

/// `(transient m)` — open a transient build over the immutable map `m`.
fn transient(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "transient", arg(args, 0))?;
    Ok(heap.alloc_transient(id))
}

/// `(assoc! t k v)` — mutate the transient in place; returns the same handle.
fn transient_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_transient(heap, "assoc!", arg(args, 0))?;
    heap.transient_assoc(id, arg(args, 1), arg(args, 2))
}

/// `(dissoc! t k)` — remove `k` from the transient in place; returns the handle.
fn transient_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_transient(heap, "dissoc!", arg(args, 0))?;
    heap.transient_dissoc(id, arg(args, 1))
}

/// `(persistent! t)` — close the transient, returning its root as an immutable map.
fn transient_persistent(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_transient(heap, "persistent!", arg(args, 0))?;
    heap.transient_persistent(id)
}

/// `(transient? x)` — true iff `x` is a transient (regardless of live state).
fn transient_pred(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Transient(_))))
}

/// `(transient-get t k [default])` — lookup against a live transient's root.
fn transient_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_transient(heap, "transient-get", arg(args, 0))?;
    let root = match heap.transient_root(id)? {
        Value::Map(m) => m,
        _ => unreachable!("a transient root is a Map"),
    };
    Ok(heap
        .map_get(root, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(transient-count t)` — entry count of a live transient, O(1).
fn transient_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_transient(heap, "transient-count", arg(args, 0))?;
    let root = match heap.transient_root(id)? {
        Value::Map(m) => m,
        _ => unreachable!("a transient root is a Map"),
    };
    Ok(Value::Int(heap.map_size(root) as i64))
}

/// `(transient-contains? t k)` — membership against a live transient's root.
fn transient_contains(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_transient(heap, "transient-contains?", arg(args, 0))?;
    let root = match heap.transient_root(id)? {
        Value::Map(m) => m,
        _ => unreachable!("a transient root is a Map"),
    };
    Ok(Value::Bool(heap.map_contains(root, arg(args, 1))))
}

fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Int(heap.string(id).chars().count() as i64)),
        _ => Err(LispError::wrong_type(heap, "string-length", "string", v)),
    }
}

/// `(display-width s)` — how many terminal/grid *cells* `s` occupies, counting
/// grapheme clusters (an emoji / flag / CJK char is 2, a combining mark 0). The
/// width-aware counterpart to `string-length` (which counts codepoints) — the
/// editor's column / cursor math uses it so a wide glyph advances two columns. The
/// GUI renderer advances the cell grid by the same measure (`crate::text_width`).
fn display_width(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Int(
            crate::text_width::display_width(heap.string(id)) as i64,
        )),
        _ => Err(LispError::wrong_type(heap, "display-width", "string", v)),
    }
}

// ---------- type reflection ----------

/// `(type-of x)` — the runtime type tag of `x` as a keyword: `:int` `:float`
/// `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
/// `:macro` `:native`. The single irreducible reflective primitive: the tag
/// predicates (`int?`/`string?`/…) are Brood wrappers over it (`std/prelude.blsp`),
/// and the in-language type checks build on it too.
fn type_of(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    // Cached keyword id per tag — `type-of` is hit per element by the seq
    // predicates, so re-interning the tag name here dominated intern cost.
    Ok(Value::Keyword(value::tag(arg(args, 0)).keyword()))
}

// ---------- value <-> text and I/O ----------

fn str_concat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut s = String::new();
    for &a in args {
        s.push_str(&printer::display(heap, a));
    }
    Ok(heap.alloc_string(&s))
}

/// `(%string-join sep coll)` — the native fast path behind `join` for a string
/// separator. Walks `coll` once, appending each element's display form (the same
/// `str`/`join` use) with `sep` between adjacent elements into one pre-sized
/// buffer — no intermediate cons list and no `reverse` pass, which is what the
/// all-Brood `join` paid (≈2N cons cells built then reversed). `coll` is realised
/// via `seq_items` (list / vector / range; empty → `""`). Semantics match the
/// prelude `join`: display form per element, separator only between adjacent
/// elements, so a single-element collection has no trailing separator.
fn string_join(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sep = match arg(args, 0) {
        s @ Value::Str(_) => printer::display(heap, s),
        v => return Err(LispError::wrong_type(heap, "%string-join", "string", v)),
    };
    let items = heap.seq_items(arg(args, 1))?;
    // Rough pre-size (separators + a small per-element allowance) to avoid most
    // re-grows without a second display pass just to compute the exact length.
    let mut s = String::with_capacity(sep.len() * items.len().saturating_sub(1) + items.len() * 8);
    for (i, &item) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(&sep);
        }
        s.push_str(&printer::display(heap, item));
    }
    Ok(heap.alloc_string(&s))
}

fn pr_str(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = printer::print(heap, arg(args, 0));
    Ok(heap.alloc_string(&s))
}

/// Start capturing the current process's output into a fresh buffer. While active,
/// `print` / terminal output ([`write_term_bytes`]) appends there instead of real
/// stdout — and so does output from any process this one `spawn`s (the capture is
/// **process-scoped and inherited**, living in the process `Ctx`; see
/// `scheduler::begin_capture`). The `nest mcp` dispatcher installs one around each
/// `tools/call` so a handler's output — even a handler run in a spawned, killable
/// process under a timeout — can't corrupt the JSON-RPC stdout stream; the captured
/// text rides back in the result envelope. Pair with [`take_captured_stdout`].
pub fn begin_stdout_capture() {
    crate::process::begin_capture();
}

/// Stop capturing and return what was written since [`begin_stdout_capture`] —
/// `Some(text)` (possibly empty) if capture was active, `None` otherwise.
pub fn take_captured_stdout() -> Option<String> {
    crate::process::take_capture()
}

/// If a capture is active on the current process, append `s` to it and return
/// `true`; otherwise `false`. The single divert point shared by `print` and
/// `write_term_bytes`.
fn capture_write(s: &str) -> bool {
    crate::process::capture_append(s)
}

/// `(%capture-begin)` — push a fresh output-capture buffer (see
/// [`begin_stdout_capture`]). The low half of the `with-out-str` macro; pairs with
/// `%capture-take`. Captures nest, so this composes with an outer MCP capture.
fn capture_begin(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    begin_stdout_capture();
    Ok(Value::Nil)
}

/// `(%capture-take)` — pop the current capture buffer and return its text as a
/// string (empty string if nothing was written), or `nil` if no capture was active
/// (see [`take_captured_stdout`]). The high half of the `with-out-str` macro.
fn capture_take(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(match take_captured_stdout() {
        Some(s) => heap.alloc_string(&s),
        None => Value::Nil,
    })
}

fn print(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    let text = parts.join(" ");
    // Divert to the capture buffer if one is active (the MCP channel must stay pure
    // JSON-RPC); otherwise write real stdout.
    let captured = capture_write(&text);
    if !captured {
        write_stdout(&text);
    }
    Ok(Value::Nil)
}

/// Write `s` to real stdout the way a well-behaved Unix tool does. A **broken
/// pipe** (the downstream consumer closed — `brood … | head`) is not a program
/// error: the `print!` macro would panic on it with a Rust backtrace + crash
/// dump (every observed `failed printing to stdout: Broken pipe` crash bottoms
/// out here), so instead we restore the terminal and exit quietly, exactly as
/// the default SIGPIPE disposition would. Any other write/flush failure is
/// best-effort-dropped (matches the old `.flush().ok()`).
fn write_stdout(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    if let Err(e) = out.write_all(s.as_bytes()).and_then(|_| out.flush()) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            restore_terminal_on_exit();
            std::process::exit(0);
        }
        // Other errors: nothing useful to do from a print primitive; drop it.
    }
}

fn eprint(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    eprint!("{}", parts.join(" "));
    use std::io::Write;
    std::io::stderr().flush().ok();
    Ok(Value::Nil)
}

/// `(%render & xs)` — the space-joined display forms of the arguments as a single
/// string (no output). The rendering half of `print`, split out so Brood's
/// `print`/`println` — which route the result through the dynamic `*out*` port —
/// hand a non-stdout sink (a buffer, a process) the exact text stdout would show.
fn render(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    Ok(heap.alloc_string(&parts.join(" ")))
}

/// `(%write-out s)` — write the ready string `s` to the current stdout sink: the
/// active capture buffer if one is set (`with-out-str`, the MCP channel), else
/// real stdout. The write half of `print` and the default value of the `*out*`
/// port — keeping it the default is what lets `with-out-str` still capture
/// un-redirected output.
fn write_out(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%write-out", arg(args, 0))?;
    if !capture_write(&s) {
        write_stdout(&s);
    }
    Ok(Value::Nil)
}

/// `(%write-err s)` — write the ready string `s` to real stderr (never captured,
/// matching `eprint`). The default value of the `*err*` port.
fn write_err(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let s = expect_string(heap, "%write-err", arg(args, 0))?;
    eprint!("{}", s);
    std::io::stderr().flush().ok();
    Ok(Value::Nil)
}

/// `(stdout-tty?)` — true when stdout is an interactive terminal, false when it's
/// captured (a pipe, a file, `cargo test`). The test framework uses this to emit
/// ANSI colour only when a human is watching, so captured output (what an LLM or
/// CI reads) stays clean plain text.
fn stdout_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::Bool(std::io::stdout().is_terminal()))
}

/// `(stdin-tty?)` — true when stdin is an interactive terminal, false when it's
/// redirected (a pipe, a file). The REPL gates raw-mode line editing on this:
/// `echo … | brood` has a piped stdin (even with a TTY stdout), so it must take
/// the plain `read-line` path, not the interactive editor.
fn stdin_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::Bool(std::io::stdin().is_terminal()))
}

// ---------- time ----------

/// `(now)` — wall-clock milliseconds since the Unix epoch, as an integer.
/// Subtract two readings to measure elapsed time (see `std/test.blsp`).
fn now(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ms))
}

/// `(now-ns)` — wall-clock nanoseconds since the Unix epoch, as an integer.
/// The fine-grained partner to `now`; subtract two readings to time sub-
/// millisecond work that `now`'s resolution would round to zero. (i64
/// nanoseconds since 1970 stays in range until the year 2262.)
fn now_ns(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ns))
}

// ---------- memory ----------

/// `(mem-bytes)` — bytes currently allocated across the whole process.
fn mem_bytes(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::live_bytes() as i64))
}

/// `(mem-peak)` — high-water mark of allocated bytes since the process started.
fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::peak_bytes() as i64))
}

/// `(gc-stats)` — a snapshot map of this process's garbage-collection activity
/// (Tier-1 observability; `docs/memory-review.md` §7). Per-process: it reports
/// the *calling* process's own LOCAL heap, never another's. Keys:
/// `:collections` (collections run since start — the automatic Stage-B
/// safepoint copies), `:copied` (cumulative LOCAL
/// objects relocated by those collections), `:reclaimed` (cumulative LOCAL
/// objects dropped), `:live` (LOCAL objects live right now), `:live-bytes` (a
/// cheap byte estimate of the LOCAL slabs — see `mem-bytes` for the process-wide
/// figure), and `:threshold` (the live count that triggers the next collection —
/// the slow/stable dial). Plus two figures for the *shared* RUNTIME code region
/// (the same for every process, not per-process): `:runtime-closures` (its total
/// promoted-closure count — grows with hot-reload churn, compacted back by the
/// safepoint, ADR-091) and `:runtime-threshold` (the count that triggers the next
/// auto-compaction). The live/reclaimable split is the expensive walk reported by
/// `(runtime-collect)`, so it's not included here.
#[cfg(feature = "dev-tools")]
fn gc_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(gc_stats_map(heap))
}

/// Build the `(gc-stats)` snapshot map of the calling process's GC activity.
/// Shared by `gc-stats` and `gc-collect` (which reports the same shape *after*
/// forcing a collection, so the delta is visible).
#[cfg(feature = "dev-tools")]
fn gc_stats_map(heap: &mut Heap) -> Value {
    let (runs, copied, reclaimed) = heap.gc_counters();
    let pairs = vec![
        (value::kw("collections"), Value::Int(runs as i64)),
        (value::kw("copied"), Value::Int(copied as i64)),
        (value::kw("reclaimed"), Value::Int(reclaimed as i64)),
        (
            value::kw("live"),
            Value::Int(heap.local_live_count() as i64),
        ),
        (
            value::kw("live-bytes"),
            Value::Int(heap.local_bytes() as i64),
        ),
        (
            value::kw("threshold"),
            Value::Int(heap.gc_threshold() as i64),
        ),
        // The shared RUNTIME code region (not per-process — every process sees the
        // same figure). `:runtime-closures` is its total promoted-closure count
        // (cheap — a slab length); it grows with hot-reload churn and the eval
        // safepoint compacts it back toward `:runtime-threshold` (single-process
        // today, ADR-091). The live/reclaimable split is the expensive walk reported
        // by `(runtime-collect)`'s `{:before :after :reclaimed}`, kept out of here.
        (
            value::kw("runtime-closures"),
            Value::Int(heap.runtime_closure_count() as i64),
        ),
        (
            value::kw("runtime-threshold"),
            Value::Int(heap.rt_gc_threshold() as i64),
        ),
        // True iff this binary was built with debug assertions (the GC tripwire /
        // verifier / poison bits are compiled in) — so a benchmark can confirm
        // it's measuring a clean release build, not a debug-armed one. `false`
        // for `make install` / `cargo build --release`.
        (
            value::kw("debug-build"),
            Value::Bool(cfg!(debug_assertions)),
        ),
    ];
    heap.map_from_pairs(pairs)
}

/// `(vm-stats)` — a snapshot map of the VM work-attribution counters (the
/// `perf-stats` feature; see `docs/benchmarking.md`). `:enabled` is `false` when
/// the binary was built without `--features perf-stats` (every other key absent —
/// the counters compiled to nothing). With the feature on: `:enabled true` plus a
/// key per counter (`:vm-apply`, `:tail-call`, `:self-tail`, `:tw-defer`,
/// `:call-ic-hit`/`:call-ic-miss`, `:global-ic-hit`/`:global-ic-miss`,
/// `:prim2-inline`/`:prim2-fallback`, `:prim1-inline`/`:prim1-fallback`,
/// `:env-get`, `:env-hops`, `:alloc`) — process-global cumulative totals across
/// every green process. The data behind the bytecode-lowering gate (ADR-096): is
/// the VM dispatch-, env-, or alloc-bound? A *counting* tool, not a timing one.
#[cfg(feature = "dev-tools")]
fn vm_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pairs = match crate::perf::snapshot() {
        Some(counters) => {
            let mut v = Vec::with_capacity(counters.len() + 1);
            v.push((value::kw("enabled"), Value::Bool(true)));
            for (name, val) in counters {
                // counter idents are snake_case; expose idiomatic kebab keywords.
                v.push((value::kw(&name.replace('_', "-")), Value::Int(val as i64)));
            }
            v
        }
        None => vec![(value::kw("enabled"), Value::Bool(false))],
    };
    Ok(heap.map_from_pairs(pairs))
}

/// `(runtime-collect)` — compact the shared RUNTIME code region now (reclaim
/// superseded hot-reload versions), returning `{:before :after :reclaimed :ran}`.
/// `:ran` is false (and nothing changes) when the runtime is shared with another
/// live process — see [`Heap::runtime_collect`]'s safety gate. Rarely needed: the
/// eval safepoint auto-compacts ([`Heap::maybe_runtime_collect`]) once churn
/// crosses the threshold; this is the explicit/force form.
#[cfg(feature = "dev-tools")]
fn runtime_collect(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (before, after, ran) = match heap.runtime_collect() {
        Some((b, a)) => (b, a, true),
        None => {
            let n = heap.runtime_closure_count();
            (n, n, false)
        }
    };
    let pairs = vec![
        (value::kw("before"), Value::Int(before as i64)),
        (value::kw("after"), Value::Int(after as i64)),
        (value::kw("reclaimed"), Value::Int((before - after) as i64)),
        (value::kw("ran"), Value::Bool(ran)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(gc-collect)` — force a collection of this process's LOCAL heap *now*,
/// returning the post-collection `(gc-stats)` map so the effect is visible.
/// An observability/test aid, **not** a load-bearing trigger: automatic
/// collection at the eval safepoint keeps memory bounded with no help from the
/// program (the removed `(hibernate)` was the load-bearing manual trigger — this
/// is not its return). Safe at any eval depth: a nullary builtin holds no
/// un-rooted LOCAL values across the collection, and every live ancestor frame
/// is already on the operand stack (ADR-061), so `collect` relocates everything
/// reachable and the freshly-built result map is allocated post-collection.
#[cfg(feature = "dev-tools")]
fn gc_collect(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    heap.collect(&mut [], &mut []);
    Ok(gc_stats_map(heap))
}

/// `(gc-trace)` / `(gc-trace on?)` — query or set per-collection GC trace
/// logging for the calling process. With no argument, returns the current state;
/// with one, sets it (truthy = on) and returns the new state. When on, each
/// minor/major collection prints a one-line summary to stderr. Per-process and
/// defaulted from the `BROOD_GC_TRACE` env var (which traces the whole run,
/// including the root process before any `(gc-trace)` call).
#[cfg(feature = "dev-tools")]
fn gc_trace(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let Some(&v) = args.first() {
        heap.set_gc_trace(crate::eval::truthy(v));
    }
    Ok(Value::Bool(heap.gc_trace()))
}

/// `(mem-limit)` — the hard memory ceiling in bytes (0 = unlimited). ADR-043.
fn mem_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::hard_limit() as i64))
}

/// `(mem-soft-limit)` — the soft memory ceiling in bytes (0 = unlimited). ADR-043.
fn mem_soft_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::soft_limit() as i64))
}

// ---------- self-hosting ----------

fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    crate::eval::eval(heap, form, root)
}

fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-string", arg(args, 0))?;
    reader::read_one_complete(heap, &s)
}

/// `(read-first s)` — parse and return the **first** form in `s`, ignoring any
/// trailing forms. The lenient sibling of `read-string`: for peeking the leading
/// form of a multi-form source (e.g. a file's `(defmodule …)` header) without
/// parsing — or erroring on — the rest.
fn read_first(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-first", arg(args, 0))?;
    reader::read_one(heap, &s)
}

/// `(read-all s)` — parse *every* form in `s` and return them as a list (empty for
/// blank/comment-only input). The all-forms sibling of `read-string` (which
/// returns only the first), and the read-half of `eval-string` without the eval —
/// so form-manipulating Brood (an editor evaluating the last sexp before point,
/// say) can isolate individual forms. Raises on a malformed/incomplete form, like
/// `read-string`; use `parse-source` for lossless, error-tolerant parsing.
fn read_all(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-all", arg(args, 0))?;
    let forms = reader::read_all(heap, &s)?;
    Ok(heap.list(forms))
}

/// `(parse-source s)` — parse s into a lossless CST tree as nested vectors, the
/// mechanism behind `std/format.blsp`. Never raises: malformed input becomes
/// `[:error "raw"]` nodes (parsing resumes after them). See `syntax::cst`.
///
/// Shape (each node is a vector `[kind …]`):
/// - Leaves carry the original source text:
///   `[:symbol "foo"]`, `[:keyword ":foo"]`, `[:int "42"]`, `[:float "1.5"]`,
///   `[:bool "true"]`, `[:nil "nil"]`, `[:str "\"hi\""]` (raw — quotes/escapes
///   included), `[:whitespace "  \n"]`, `[:comment ";; hi\n"]`, `[:error "raw"]`.
/// - Reader macros wrap a single child form:
///   `[:quote child]`, `[:quasi child]`, `[:unquote child]`, `[:splice child]`.
/// - Containers carry a child vector:
///   `[:root [child …]]`, `[:list [child …]]`, `[:vector [child …]]`,
///   `[:map [child …]]`.
///
/// Roundtrip property: concatenating every leaf's text in tree order reproduces
/// the input — this is what makes the CST a faithful basis for formatting.
fn parse_source(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "parse-source", arg(args, 0))?;
    let root = cst::parse(&s);
    Ok(cst_to_value(heap, &root, &s))
}

fn cst_to_value(heap: &mut Heap, node: &cst::Node, src: &str) -> Value {
    use cst::NodeKind::*;
    let tag = |k: &'static str| Value::Keyword(value::intern(k));
    match node.kind {
        // Leaves: [kind raw-text].
        Symbol | Keyword | Int | Float | Str | Bool | Nil | Whitespace | Comment | Error => {
            let k = match node.kind {
                Symbol => "symbol",
                Keyword => "keyword",
                Int => "int",
                Float => "float",
                Str => "str",
                Bool => "bool",
                Nil => "nil",
                Whitespace => "whitespace",
                Comment => "comment",
                Error => "error",
                _ => unreachable!(),
            };
            let text = heap.alloc_string(node.text(src));
            heap.alloc_vector(vec![tag(k), text])
        }
        // Reader-macro wrappers: [kind child]. The single structural child is
        // the wrapped form; any leading whitespace child is dropped (the wrapper
        // owns its position via its parent's children list).
        Quote | Quasi | Unquote | Splice => {
            let k = match node.kind {
                Quote => "quote",
                Quasi => "quasi",
                Unquote => "unquote",
                Splice => "splice",
                _ => unreachable!(),
            };
            // A reader-macro node's children are the wrapped form's parse
            // result(s) — usually a single form. Walk and pick the first
            // non-trivia child; nest the rest as following siblings would be a
            // parse bug, but in case of empty (EOF after ~/`/'/), emit nil.
            let child = node
                .forms()
                .next()
                .map(|c| cst_to_value(heap, c, src))
                .unwrap_or(Value::Nil);
            heap.alloc_vector(vec![tag(k), child])
        }
        // Containers: [kind [child …]]. Children include trivia (whitespace +
        // comments) so the formatter can preserve blank-line + comment intent.
        Root | List | Vector | Map => {
            let k = match node.kind {
                Root => "root",
                List => "list",
                Vector => "vector",
                Map => "map",
                _ => unreachable!(),
            };
            let kids: Vec<Value> = node
                .children
                .iter()
                .map(|c| cst_to_value(heap, c, src))
                .collect();
            let kids_vec = heap.alloc_vector(kids);
            heap.alloc_vector(vec![tag(k), kids_vec])
        }
    }
}

/// `(parse-source-positioned s)` — like `parse-source`, but every CST node is a
/// MAP carrying its absolute source position rather than a `[kind …]` vector:
/// `{:kind :start :end}` for leaves (plus `:text`, the leaf's raw source), and
/// additionally `:kids` (a vector of child node maps) for containers
/// (`:root`/`:list`/`:vector`/`:map`) and reader-macro wrappers
/// (`:quote`/`:quasi`/`:unquote`/`:splice`). `:start`/`:end` are half-open
/// CHARACTER offsets (not bytes) — matching `string-length` and editor buffer
/// point — so structural tooling (`std/sexp`) navigates the tree directly.
///
/// The kernel already tracks every node's span; this projects it in one pass. It
/// exists because recovering those positions in interpreted Brood (`std/sexp`'s
/// former `annotate` walk) was O(n) and dominated structural-navigation latency.
fn parse_source_positioned(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "parse-source-positioned", arg(args, 0))?;
    let root = cst::parse(&s);
    let b2c = byte_to_char_offsets(&s);
    Ok(cst_to_positioned(heap, &root, &s, &b2c))
}

/// Per-byte → character-offset table for `s`: `t[b]` is the count of characters
/// before byte offset `b`. Length `s.len() + 1` so a node's `span.end` (which can
/// equal `s.len()`) is indexable. CST spans land on char boundaries; a byte
/// interior to a multi-byte char maps to that char's own index (never queried).
fn byte_to_char_offsets(s: &str) -> Vec<u32> {
    let mut t = vec![0u32; s.len() + 1];
    let mut byte = 0usize;
    let mut ci = 0u32;
    for ch in s.chars() {
        let w = ch.len_utf8();
        for k in 0..w {
            t[byte + k] = ci;
        }
        byte += w;
        ci += 1;
    }
    t[s.len()] = ci;
    t
}

fn cst_node_kind_name(kind: cst::NodeKind) -> &'static str {
    use cst::NodeKind::*;
    match kind {
        Symbol => "symbol",
        Keyword => "keyword",
        Int => "int",
        Float => "float",
        Str => "str",
        Bool => "bool",
        Nil => "nil",
        Whitespace => "whitespace",
        Comment => "comment",
        Error => "error",
        Quote => "quote",
        Quasi => "quasi",
        Unquote => "unquote",
        Splice => "splice",
        Root => "root",
        List => "list",
        Vector => "vector",
        Map => "map",
    }
}

fn cst_to_positioned(heap: &mut Heap, node: &cst::Node, src: &str, b2c: &[u32]) -> Value {
    use cst::NodeKind::*;
    let kw = |k: &'static str| Value::Keyword(value::intern(k));
    let start = Value::Int(b2c[node.span.start as usize] as i64);
    let end = Value::Int(b2c[node.span.end as usize] as i64);
    let mut pairs: Vec<(Value, Value)> = vec![
        (kw("kind"), kw(cst_node_kind_name(node.kind))),
        (kw("start"), start),
        (kw("end"), end),
    ];
    match node.kind {
        // Leaves carry their raw source text; positions alone make them navigable.
        Symbol | Keyword | Int | Float | Str | Bool | Nil | Whitespace | Comment | Error => {
            let text = heap.alloc_string(node.text(src));
            pairs.push((kw("text"), text));
        }
        // Containers + wrappers carry their (position-annotated) children — trivia
        // included, exactly as `parse-source`, so callers filter what they want.
        Quote | Quasi | Unquote | Splice | Root | List | Vector | Map => {
            let kids: Vec<Value> = node
                .children
                .iter()
                .map(|c| cst_to_positioned(heap, c, src, b2c))
                .collect();
            let kids_vec = heap.alloc_vector(kids);
            pairs.push((kw("kids"), kids_vec));
        }
    }
    heap.map_from_pairs(pairs)
}

/// `(tree-sitter-parse source lang)` — parse a foreign language into the same
/// positioned-CST node shape as `parse-source-positioned`. Mechanism lives in
/// `crate::treesit` (feature-gated); this just unwraps the args. See §C.
fn tree_sitter_parse(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "tree-sitter-parse", arg(args, 0))?;
    let lang = match arg(args, 1) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "tree-sitter-parse",
                "keyword",
                v,
            ))
        }
    };
    crate::treesit::parse(heap, &src, &lang)
}

/// `(reload-defs path)` — like `load`, but only re-evaluates **definitions**
/// (`def`/`defmacro` and `def…`-named macros: `defn`, `defmodule`, `defdyn`,
/// `defonce`, user definers). All other top-level forms — `(require …)`,
/// `(load …)`, a `(main-loop 0)` entry call — are silently skipped. Used by the
/// file watcher (`std/reload.blsp`): on the **second** and subsequent visits to
/// a file we want to refresh the code (so the running program sees the new
/// behaviour via late binding) but **not** re-run side-effecting top-level calls
/// — re-executing those would spawn a duplicate long-running process (a
/// tail-recursive loop) or block the watcher itself.
///
/// **Atomicity:** the whole file is read before any form is evaluated, so a
/// half-saved / syntactically broken file applies *zero* defs (read fails
/// first). Forms are then expanded+evaluated one at a time, exactly like
/// `load`, so a macro a form defines is visible to later forms in the same file
/// (`lib.rs`). The residual non-atomic window is a *runtime* error while
/// evaluating form N, after 1..N-1 already landed; full snapshot/rollback is
/// deferred (docs/live-editing.md Stage 2). Returns `nil`. ADR-013 hot reload's
/// mechanism flowing through to the tool layer.
fn reload_defs(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "reload-defs", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("reload-defs: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    let prev = heap.set_current_file(Some(path.clone()));
    // Namespace bracketing + forward-ref pre-scan, like `load` (ADR-065): a
    // reloaded namespaced file re-establishes its own namespace (its `(ns …)` form
    // is re-evaluated below) so its re-saved defs are qualified correctly.
    let prev_ns = heap.set_compile_ns(None);
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let known = if crate::eval::macros::file_opens_ns(heap, &form_vals) {
        crate::eval::macros::scan_def_names(heap, &form_vals)
    } else {
        std::collections::HashSet::new()
    };
    let prev_known = heap.set_ns_known_names(known);
    let prev_imports = heap.set_imports(std::collections::HashMap::new());
    let mut result = Ok(Value::Nil);
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds; re-fetch
    // each from the (relocated) root stack rather than the stale `forms` Vec. Same
    // discipline as `load`.
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        // Re-eval only *definitions*; skip side-effecting top-level forms
        // (`(require …)`, `(load …)`, a `(main-loop 0)` entry call). A form is a
        // definition when its head symbol starts with "def" **and** is actually a
        // definer — one of the `def`/`defmacro` core special forms, or a symbol
        // currently bound to a macro (`defn`/`defmodule`/`defdyn`/`defonce` and
        // any user `def…` macro). The macro check drops the false positive on a
        // plain top-level *call* to a function whose name merely starts with
        // "def" (e.g. `(default-config)`): that head resolves to a `Fn`, not a
        // macro, so it's correctly skipped.
        //
        // Known limitation (accepted — docs/live-editing.md Stage 2): a definer
        // macro *not* named `def…` (e.g. `(register-handler …)` expanding to a
        // `def`) is skipped. Workaround: prefix definer macros with `def`, the
        // Lisp convention anyway. (`require` skipping is likewise intentional: we
        // don't transitively reload other modules; the user watches each path
        // explicitly with `reload-on-change`.)
        let head_is_def = match form {
            Value::Pair(p) => {
                let (head, _) = heap.pair(p);
                match head {
                    Value::Sym(s) => {
                        let nm = value::symbol_name(s);
                        // The `(defmodule …)` header is re-evaluated too (so the
                        // reloaded file's namespace + imports are re-established for
                        // its defs, ADR-065) — it's a `def…`-named macro, caught here.
                        //
                        // Resolve the head through the current namespace + imports
                        // before the macro check, so a *module-qualified* definer
                        // macro used bare (e.g. `deflive` from `(:use web/live)`,
                        // bound as `web/live/deflive`, not in root) is still
                        // recognised and re-evaluated. Without this, a `(deflive …)`
                        // top-level form would be skipped and its defs never reload.
                        nm.starts_with("def")
                            && (nm == "def"
                                || nm == "defmacro"
                                || {
                                    let resolved =
                                        crate::eval::macros::resolve_reference(heap, s);
                                    matches!(
                                        heap.env_get(root, resolved),
                                        Some(Value::Macro(_))
                                    )
                                })
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !head_is_def {
            continue;
        }
        // Same def-site recording / expand / eval shape as `load` for the
        // forms we *do* evaluate, so cross-file goto still lands at the
        // re-saved def site.
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    result.map(|_| Value::Nil)
}

fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "load", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("load: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    // Read positioned so errors point at a line; tag every error with the file
    // (`FILE:LINE:COL:`, see docs/tooling.md).
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    // Expose the file to Brood (`(current-file)`) for the duration of the load,
    // so the test macros can record each test's source location; restore the
    // previous file afterward since loads nest.
    let prev = heap.set_current_file(Some(path.clone()));
    // A loaded file starts at the root namespace and its own `(ns …)` form sets the
    // current namespace for the rest of the file (ADR-065); restore the caller's
    // namespace afterward so loads nest and ns state never leaks out of a file.
    let prev_ns = heap.set_compile_ns(None);
    // Forward-reference pre-scan (ADR-065): if the file opens a namespace, record
    // the bare names it will define so a reference to a later definition resolves.
    // Cheap (read-only, no GC), gated on the file actually using `(ns …)`.
    let form_vals: Vec<Value> = forms.iter().map(|(f, _)| *f).collect();
    let known = if crate::eval::macros::file_opens_ns(heap, &form_vals) {
        crate::eval::macros::scan_def_names(heap, &form_vals)
    } else {
        std::collections::HashSet::new()
    };
    let prev_known = heap.set_ns_known_names(known);
    let prev_imports = heap.set_imports(std::collections::HashMap::new());

    // **Bounded loading — the core memory guarantee (docs/memory-review.md).**
    // The collector now reclaims at ANY eval depth (ADR-061), so a file loaded
    // here is bounded no matter how deep `(load …)` sits — no `GcBlockReset`
    // depth-1 trick is needed any more. We still root the unevaluated forms across
    // the per-form eval: a collection during form `i` relocates the LOCAL forms
    // `i+1..` this loop still holds, so we re-fetch each from the (relocated) root
    // stack via `root_at` rather than the stale `forms` Vec. (Living in `load`,
    // the core, means every entry path — `brood`, `nest`, MCP `eval`, the future
    // editor — inherits the bound for free.)
    let mut result = Ok(Value::Nil);
    let base = heap.roots_len();
    for (form, _) in &forms {
        heap.push_root(*form);
    }
    for (i, &(_, pos)) in forms.iter().enumerate() {
        let form = heap.root_at(base + i);
        heap.note_definition(form, pos);
        result = crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.truncate_roots(base);
    heap.set_current_file(prev);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    result
}

/// `(eval-string "src")` — read and evaluate every form in a string against the
/// global environment. Inherits the current namespace (ADR-065): the REPL evaluates
/// each entry through here, so a `(ns foo)` typed at the REPL sticks to later
/// entries. To load a *module* source at the root namespace, use `%load-string`.
fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "eval-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, false)
}

/// `(%load-string "src")` — the string analogue of `load`: read+eval every form,
/// but bracket the current namespace (reset to root, restore the caller's after),
/// so an embedded module's own `(ns …)` governs it and ns state doesn't leak to the
/// caller. Used by `require-one` for baked-in std modules (ADR-065).
fn load_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%load-string", arg(args, 0))?;
    eval_string_inner(heap, env, &src, true)
}

/// Shared body of `eval-string` / `%load-string`. When `reset_ns`, the current
/// namespace is reset to root for the duration and the caller's restored after.
fn eval_string_inner(heap: &mut Heap, env: EnvId, src: &str, reset_ns: bool) -> LispResult {
    let root = heap.env_root(env);
    let forms = reader::read_all(heap, src)?;
    // When loading a module (`reset_ns`), bracket the namespace at root and
    // pre-scan its def heads for forward references; the plain `eval-string` (REPL,
    // inline) inherits the current namespace and does neither (ADR-065).
    let (prev_ns, prev_known, prev_imports) = if reset_ns {
        let pn = heap.set_compile_ns(None);
        let known = if crate::eval::macros::file_opens_ns(heap, &forms) {
            crate::eval::macros::scan_def_names(heap, &forms)
        } else {
            std::collections::HashSet::new()
        };
        let pk = heap.set_ns_known_names(known);
        let pi = heap.set_imports(std::collections::HashMap::new());
        (Some(pn), Some(pk), Some(pi))
    } else {
        (None, None, None)
    };
    // Root the unevaluated forms across the per-form eval — a collection at any
    // depth (ADR-061) relocates the LOCAL forms this loop still holds.
    let base = heap.roots_len();
    for &form in &forms {
        heap.push_root(form);
    }
    let mut result: LispResult = Ok(Value::Nil);
    for i in 0..forms.len() {
        let form = heap.root_at(base + i);
        match crate::eval::macros::compile(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
        {
            Ok(v) => result = Ok(v),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    heap.truncate_roots(base);
    if let Some(pn) = prev_ns {
        heap.set_compile_ns(pn);
    }
    if let Some(pk) = prev_known {
        heap.set_ns_known_names(pk);
    }
    if let Some(pi) = prev_imports {
        heap.set_imports(pi);
    }
    result
}

/// Standard-library modules baked into the binary (like the prelude), so they load
/// from any directory with no file paths. The require / provide / load-path
/// *policy* is written in Brood (`std/prelude.blsp`, ADR-019); Rust only exposes
/// an embedded module's source here, via `%builtin-module` (ADR-006/008).
///
/// Split into [`CORE_MODULES`] (always baked in) and [`DEV_MODULES`] (only under
/// the `dev-tools` feature), so a `nest release` lean runtime
/// (`--no-default-features`) carries no test/observer/tooling/REPL source
/// (ADR-038, docs/release.md). `builtin_module` consults both.
const CORE_MODULES: &[(&str, &str)] = &[
    // Output ports: the redirectable sink behind print/println — a port is a 1-arg
    // string sink, with `process-port`/`fn-port` + `with-out`/`with-err`. Pairs
    // with the prelude's `*out*`/`*err*` dynamic vars. Opt-in, no dependencies.
    ("io", include_str!("../../../std/io.blsp")),
    // Fuzzy (subsequence) string matching + ranking: `fuzzy-match` / `fuzzy-filter`,
    // the matcher completion UIs ride on. Pure Brood, no dependencies. Opt-in.
    ("fuzzy", include_str!("../../../std/fuzzy.blsp")),
    // Plain-text utilities (pure string->string): `fill` greedy word-wraps to a column
    // width — the engine behind an editor's fill-paragraph / M-q, and reusable for
    // wrapping help text or terminal output. No dependencies. Opt-in.
    ("text", include_str!("../../../std/text.blsp")),
    ("project", include_str!("../../../std/tool/project.blsp")),
    // The package manager (ADR-037): resolves the manifest's :dependencies into a
    // lock file + load-path entries. Required lazily by `project-setup` only when a
    // project actually declares deps. Opt-in, never in the prelude.
    ("package", include_str!("../../../std/tool/package.blsp")),
    // TCP sockets (ADR-062): active-socket helpers + a spawn-per-connection
    // server over the non-blocking tcp-* primitives. Opt-in, never in the prelude.
    ("net/tcp", include_str!("../../../std/net/tcp.blsp")),
    // The file & filesystem library: whole-file/line I/O, directory walking, path
    // helpers — Brood over the fs primitives. Opt-in, never in the prelude.
    ("file", include_str!("../../../std/file.blsp")),
    // A minimal HTTP/1.0 server (ADR-062) over the tcp + file libraries — request
    // parsing, response rendering, a router, static files. Opt-in.
    ("net/http", include_str!("../../../std/net/http.blsp")),
    // JSON ↔ Brood data, written entirely in Brood (a recursive-descent parser +
    // encoder over the string primitives; the reader's `\u{}` escape is the
    // codepoint→char mechanism). Opt-in, never in the prelude.
    ("json", include_str!("../../../std/json.blsp")),
    // Server-Sent Events (text/event-stream): a client reader process that streams
    // events to a subscriber's mailbox (pairs with ui's `with-events`) + server-side
    // framing. Pure frame parsing + a thin IO loop over tcp; reuses http's URL/header
    // helpers. Opt-in.
    ("net/sse", include_str!("../../../std/net/sse.blsp")),
    // The process framework, bundled in the default install (ADR-085 amended —
    // batteries-included, not externalized). `proc/gen` is the gen_server-style
    // server loop (`defprocess` / `spawn-server` / `!` / `gen-call` / `stop`); the
    // core `log` module is a `proc/gen` process. `proc/supervisor` is OTP-style
    // supervision — independent of `proc/gen`, both over the same kernel primitives.
    ("proc/gen", include_str!("../../../std/proc/gen.blsp")),
    (
        "proc/supervisor",
        include_str!("../../../std/proc/supervisor.blsp"),
    ),
    // Order a flat process-info snapshot as a parent→child forest (depth-tagged, DFS
    // by id). A pure, dependency-free transform — CORE, not dev-tools: it's shared by
    // the dev observer's tree sort *and* a shipped app's process list (myedit's
    // *Process List*), so a `nest release` binary needs it baked in.
    ("proctree", include_str!("../../../std/tool/proctree.blsp")),
    // Run a thunk off the current process with an optional timeout + cancel
    // (ADR-006): `task` (async, tagged-reply handle), `cancel-task`, and the
    // synchronous `await`. Pure Brood over spawn / receive / exit — the generic
    // version of the editor's hand-rolled async-eval watchdog. Opt-in.
    ("task", include_str!("../../../std/task.blsp")),
    // An async, safe logger (ADR-006): a `proc/gen` process holding a list of
    // backends, each an `io` port + a min level + a formatter. Log calls are casts
    // (fire-and-forget = async); the one process serialises writes (no interleaving)
    // and isolates a backend crash. Opt-in, never in the prelude.
    ("log", include_str!("../../../std/log.blsp")),
    // Erlang :telemetry-style instrumentation (ADR-106). Handlers run in a dedicated
    // LISTENER process (emit is a fire-and-forget send), so a buggy handler can never
    // crash/hang the emitting process — only the listener, which a throwing handler
    // doesn't even do (caught + detached). The handler table is a `def`-rebound global
    // that survives a listener restart (ADR-013). `span` brackets a body with
    // :start/:stop/:exception events; `forward` runs handler work in your own process.
    // Opt-in, never in the prelude.
    ("telemetry", include_str!("../../../std/telemetry.blsp")),
    // Date and time utilities (UTC): epoch↔datetime conversion, ISO 8601
    // format/parse, arithmetic, calendar predicates. Pure Brood over `now`.
    ("datetime", include_str!("../../../std/datetime.blsp")),
    // Hex and Base64 encoding/decoding. Pure Brood over `char->int` /
    // `string->utf8-bytes` / `utf8-bytes->string`. Opt-in, never in the prelude.
    ("encoding", include_str!("../../../std/encoding.blsp")),
    // Descriptive statistics over numeric sequences: mean, median, stddev,
    // variance, percentile, mode, frequencies. Pure Brood over sort/fold/sqrt.
    ("stats", include_str!("../../../std/stats.blsp")),
    // Pull-stream protocol + combinators over green processes. Sources: list,
    // fn-generator, range, TCP socket. Transformers: map/filter/take/drop/
    // take-while/chunk/concat/lines. Terminals: fold/to-list/to-vector/
    // for-each/pipe/to-socket. Foundation for the HTTP streaming layer.
    ("stream", include_str!("../../../std/stream.blsp")),
    // URL encoding/decoding and parsing: percent-encode/decode, query-string
    // encode/decode, parse-url, build-url. Pure Brood over string primitives.
    ("url", include_str!("../../../std/url.blsp")),
    // CSV parsing and emitting: csv-parse, csv-parse-maps, csv-emit,
    // csv-emit-maps. Handles quoted fields, escaped quotes, \r\n endings.
    ("csv", include_str!("../../../std/csv.blsp")),
    // RFC 4122 version-4 UUID generation via the OS CSPRNG (random-token).
    // uuid-v4, uuid-nil, uuid?.
    ("uuid", include_str!("../../../std/uuid.blsp")),
    // {{var}} string templating: render a template string against a data map.
    // render, render-all.
    ("template", include_str!("../../../std/template.blsp")),
    // Purely functional FIFO queue (two-list, amortised O(1)) and min-priority
    // queue (sorted-list, O(n) insert / O(1) pop).
    ("queue", include_str!("../../../std/queue.blsp")),
    // Multi-valued map: one key may hold multiple values (a map of lists).
    // multimap-assoc, multimap-get, multimap-get-all, multimap-dissoc, …
    ("multimap", include_str!("../../../std/multimap.blsp")),
    // SHA-256 and HMAC-SHA256. sha256 is a clean alias for %sha256;
    // hmac-sha256 is pure Brood over %sha256 per RFC 2104; hash-string is djb2.
    ("hash", include_str!("../../../std/hash.blsp")),
    // LCS-based sequence diff: diff-seq, diff-lines, diff-summary, diff-patch,
    // diff-unified. O(m*n) time/space; suitable for small-to-medium sequences.
    ("diff", include_str!("../../../std/diff.blsp")),
    // Path string manipulation: join, split, basename, dirname, extension, stem,
    // normalize, relative-to. Consolidates the prelude's path-* globals under
    // a single path/ namespace with additional operations.
    ("path", include_str!("../../../std/path.blsp")),
    // OS/process interface: env vars, argv, subprocess execution, OS type, halt.
    // Wraps the %env-all/%argv/%os-cmd/%os-type/%halt primitives with a clean API.
    ("system", include_str!("../../../std/system.blsp")),
    // Authenticated encryption (ChaCha20-Poly1305), PBKDF2 key derivation, secure
    // random bytes. Wraps the %chacha20-* and %pbkdf2-sha256 primitives.
    ("crypto", include_str!("../../../std/crypto.blsp")),
    // Process-backed state cell: start/get/update/get-and-update/cast/stop.
    // A thin Brood layer over spawn/send/receive for the common "stateful process" case.
    ("agent", include_str!("../../../std/agent.blsp")),
    // The editor framework's buffer model (M2 Phase 1, ADR-045): an immutable
    // buffer over the rope primitives, opt-in, never in the prelude.
    (
        "editor/buffer",
        include_str!("../../../std/editor/buffer.blsp"),
    ),
    // The display/input seam (M3, ADR-046): `display` is the render-op protocol
    // (pure data constructors); `keymap` is the rebindable key→command dispatcher
    // shared by the line editor and the observer; `observer` is a process-viewer
    // built on them + the `term-*`/`gui-*` primitives. All opt-in, never in the prelude.
    // The shared named-face / theme registry (the counterpart to `keymap`): style
    // named once, referenced everywhere, restyled in one place. Required by `ui`
    // (so every ui-run app gets it) and the observer.
    ("editor/face", include_str!("../../../std/editor/face.blsp")),
    (
        "editor/display",
        include_str!("../../../std/editor/display.blsp"),
    ),
    (
        "editor/keymap",
        include_str!("../../../std/editor/keymap.blsp"),
    ),
    // Composable, runtime-reconfigurable behaviour layers over `keymap` (the
    // generic mechanism the editor's "modes" are built from; buffer-agnostic).
    // Opt-in, never in the prelude. See docs/layers.md.
    (
        "editor/layers",
        include_str!("../../../std/editor/layers.blsp"),
    ),
    // Structural (s-expression) navigation over the parse-source CST — reusable
    // Brood-code tooling (same tier as the formatter / LSP), not editor-specific.
    // (The text-mode/brood-mode *layers* built on it are editor policy and live in
    // the editor app — examples/editor/src/ — not here.) Opt-in. (docs/layers.md)
    ("sexp", include_str!("../../../std/tool/sexp.blsp")),
    // A small backtracking regular-expression engine, pure Brood (literals, ., * + ?,
    // ^ $, [...] sets, \d \w \s, |, groups; no ranges/captures yet). Opt-in.
    ("regex", include_str!("../../../std/regex.blsp")),
    ("editor/ui", include_str!("../../../std/editor/ui.blsp")),
    // Serve a `ui-run` app to remote frontends — the Emacs `--daemon`/`emacsclient`
    // model (ADR-090): the app runs on the daemon, a thin `attach` client paints
    // pushed frames + ships back keys. Pure Brood over `ui-run` + the node link.
    (
        "editor/serve",
        include_str!("../../../std/editor/serve.blsp"),
    ),
    // Emacs-style tiled window splits: an immutable binary layout tree + pure
    // pane/divider geometry + drag-to-resize over `:drag` mouse events (ADR-077).
    // Reusable editor toolkit (content-agnostic); the keybindings + payload are
    // editor policy. Opt-in, never in the prelude.
    ("editor/pane", include_str!("../../../std/editor/pane.blsp")),
    // Bare ANSI escape *strings* for simple terminal scripts (`print` them
    // directly) — the lightweight counterpart to the `display` render-op
    // protocol. Opt-in, never in the prelude.
    ("editor/ansi", include_str!("../../../std/editor/ansi.blsp")),
    // Sets as a library over maps (ADR-062): a set is a map of `element → true`,
    // so membership/elements/size reuse `contains?`/`keys`/`count`; the module
    // adds `set`/`conj`/`disj`/`union`/`intersection`/`difference`/`subset?`.
    // Opt-in, never in the prelude (no `#{…}` literal / distinct type yet).
    ("set", include_str!("../../../std/set.blsp")),
    // The interactive REPL line editor (ADR-052): `highlight` is the pure lexical
    // syntax-highlighter / bracket-matcher / signature + completion scanners;
    // `lineedit` is the raw-mode, emacs-style editor built on it + the inline
    // `term-*` seam. Both opt-in, never in the prelude; `repl` requires them.
    // `highlight`/`lineedit` stay in CORE: they are reusable UI a shipped app may
    // `require` (the editor's minibuffer reuses `std/lineedit`'s core), not just
    // REPL plumbing — so a lean release keeps them.
    (
        "editor/highlight",
        include_str!("../../../std/editor/highlight.blsp"),
    ),
    // Generic tree-sitter language services (`fontify` + structural motions) over
    // the `tree-sitter-parse` builtin's positioned CST — the foreign-language
    // analogue of `sexp`+`highlight`. Pure UI a shipped editor `require`s for its
    // ruby/elixir/… modes (ROADMAP §C), so it stays in CORE; opt-in, never prelude.
    (
        "editor/treesit",
        include_str!("../../../std/editor/treesit.blsp"),
    ),
    // Lexical Markdown highlighter — the `highlight` analogue for `.md` buffers
    // (`markdown-spans` → `[start end face]` spans, ADR-092). Pure UI a shipped app
    // may `require` (myedit's markdown-mode), so it stays in CORE alongside
    // `highlight`/`lineedit`; opt-in, never in the prelude.
    (
        "editor/markdown",
        include_str!("../../../std/editor/markdown.blsp"),
    ),
    // Lexical `.env` and Dockerfile highlighters, the dotenv/Dockerfile analogues of
    // `markdown` (`env-spans` / `dockerfile-spans` → `[start end face]` spans). Pure
    // UI a shipped app may `require` (myedit's env-/docker-mode); CORE, like markdown.
    (
        "editor/dotenv",
        include_str!("../../../std/editor/dotenv.blsp"),
    ),
    (
        "editor/dockerfile",
        include_str!("../../../std/editor/dockerfile.blsp"),
    ),
    (
        "editor/lineedit",
        include_str!("../../../std/editor/lineedit.blsp"),
    ),
    ("format", include_str!("../../../std/format.blsp")),
];

/// Dev/tooling modules — baked in only under the `dev-tools` feature (the dev
/// `brood`/`nest` + tests). A `nest release` lean runtime
/// (`--no-default-features`) omits them, so a shipped app carries no test
/// framework, process observer, MCP/doc/hot-reload tooling, or interactive REPL
/// (ADR-038, docs/release.md). `project` stays in CORE — it boots the bundle;
/// `lineedit`/`highlight` stay too (reusable UI, e.g. the editor's minibuffer).
#[cfg(feature = "dev-tools")]
const DEV_MODULES: &[(&str, &str)] = &[
    // The test framework — `deftest`/`describe`/`assert=`/`is`. Never shipped.
    ("test", include_str!("../../../std/tool/test.blsp")),
    // Doc generation (`nest doc`) — tooling, not runtime.
    ("docs", include_str!("../../../std/tool/docs.blsp")),
    // Generate editor syntax grammars (VS Code TextMate, Emacs font-lock) from the
    // language's own `(special-forms)` — one source of truth, no drift (ADR-092).
    ("grammar", include_str!("../../../std/tool/grammar.blsp")),
    // The process viewer / debug tooling (`nest observe`, `(observe)`).
    ("observer", include_str!("../../../std/tool/observer.blsp")),
    // The hot-reload file watcher — a dev-loop convenience.
    ("reload", include_str!("../../../std/tool/reload.blsp")),
    // The Model Context Protocol tool surface — `(mcp-tools)` returns the
    // catalogue the `nest mcp` dispatcher reads (ADR-036, docs/mcp.md, step 3).
    ("mcp", include_str!("../../../std/tool/mcp.blsp")),
    // The read-eval-print loop itself, written in Brood (`(require 'repl)`):
    // policy over the `read-line`/`eval-string`/`pr-str` primitives. The Rust
    // binaries (`brood`, `nest repl`) just bootstrap into `(repl-run)`. A shipped
    // app runs its own `:main`, never the REPL.
    ("repl", include_str!("../../../std/tool/repl.blsp")),
];

/// Empty in a lean (`--no-default-features`) release runtime — the dev modules
/// above are not compiled in at all (their `include_str!` never runs).
#[cfg(not(feature = "dev-tools"))]
const DEV_MODULES: &[(&str, &str)] = &[];

/// Baked-in reference *documents* (markdown), the counterpart to
/// [`EMBEDDED_MODULES`] for non-module text. `(%builtin-doc 'brood-for-claude)`
/// returns the language guide that `nest new` scaffolds into each new project,
/// so a freshly-scaffolded project is self-contained without depending on a
/// Brood install path.
const EMBEDDED_DOCS: &[(&str, &str)] = &[
    (
        "brood-for-claude",
        include_str!("../../../docs/brood-for-claude.md"),
    ),
    // The Claude Code skill that `nest new` drops into each project's
    // `.claude/skills/`, so an AI assistant editing the project auto-loads the
    // Brood-writing rules. The full reference is `brood-for-claude`; this is the
    // short triggerable checklist (`SKILL.md` frontmatter + the LLM traps).
    // Canonical source lives here in `docs/` (a tracked path); the repo's own
    // `.claude/skills/writing-brood/SKILL.md` is a local symlink to it — `.claude/`
    // is gitignored, and a compile-time `include_str!` must not depend on an
    // untracked path (it would break a fresh clone's build).
    (
        "writing-brood-skill",
        include_str!("../../../docs/writing-brood-skill.md"),
    ),
];

/// Coerce a (symbol | keyword | string) name argument to its spelling, the shape
/// every embedded-source lookup accepts. `None` for any other value.
fn embedded_name(heap: &Heap, v: Value) -> Option<String> {
    match v {
        Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    }
}

/// The lookup body shared by `%builtin-module` and `%builtin-doc`: coerce the
/// (symbol | keyword | string) name argument, find it in `table`, return the
/// baked-in source as a fresh string (or `nil` if absent). `who`/`label` are
/// used only in the type-error message.
fn lookup_embedded(
    args: &[Value],
    heap: &mut Heap,
    table: &[(&str, &str)],
    who: &'static str,
    label: &'static str,
) -> LispResult {
    let v = arg(args, 0);
    let name = match embedded_name(heap, v) {
        Some(name) => name,
        None => return Err(LispError::wrong_type(heap, who, label, v)),
    };
    match table.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok(heap.alloc_string(src)),
        None => Ok(Value::Nil),
    }
}

/// `(%builtin-module name)` — the source of a baked-in std module as a string,
/// or nil if there is none. Mechanism only: `require` (Brood) consults this
/// before searching the load-path.
fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Core modules first, then dev/tooling modules (absent in a lean release
    // runtime). Both go through `lookup_embedded`, which also validates the arg.
    let found = lookup_embedded(args, heap, CORE_MODULES, "%builtin-module", "module name")?;
    if !matches!(found, Value::Nil) {
        return Ok(found);
    }
    let found = lookup_embedded(args, heap, DEV_MODULES, "%builtin-module", "module name")?;
    if !matches!(found, Value::Nil) {
        return Ok(found);
    }
    // Not a baked-in std module — consult a mounted release bundle (the app's
    // own modules + bundled deps), so `require` resolves them with no change to
    // its load-path logic (ADR-038). The arg type was already validated above.
    let name = match embedded_name(heap, arg(args, 0)) {
        Some(name) => name,
        None => return Ok(Value::Nil),
    };
    match crate::bundle::mounted() {
        Some(b) => match b.module_src(&name) {
            Some(src) => Ok(heap.alloc_string(src)),
            None => Ok(Value::Nil),
        },
        None => Ok(Value::Nil),
    }
}

/// `(%bundled?)` — true when this executable is a release bundle (an app built
/// by `nest release`), false for a plain `brood`/`nest` runtime.
fn bundled_p(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(crate::bundle::is_bundled()))
}

/// `(%bundle-manifest)` — the embedded `project.blsp` source of a release
/// bundle, or nil when not bundled.
fn bundle_manifest(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::bundle::mounted() {
        Some(b) => Ok(heap.alloc_string(&b.manifest)),
        None => Ok(Value::Nil),
    }
}

/// `(%bundle-module-names)` — the list of module names (filename stems) embedded
/// in a release bundle, or nil when not bundled.
fn bundle_module_names(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::bundle::mounted() {
        Some(b) => {
            let items: Vec<Value> = b.module_names().map(|n| heap.alloc_string(n)).collect();
            Ok(heap.list(items))
        }
        None => Ok(Value::Nil),
    }
}

/// `(%builtin-doc name)` — the source of a baked-in reference document as a
/// string, or nil if there is none. Used by `nest new` to scaffold the language
/// guide into each new project.
fn builtin_doc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(args, heap, EMBEDDED_DOCS, "%builtin-doc", "doc name")
}

/// `(name x)` — the spelling of a symbol or keyword as a string (no leading `:`),
/// or the string unchanged. The module system uses it to turn a module name into
/// a filename.
fn name_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(s) | Value::Keyword(s) => Ok(heap.alloc_string(&value::symbol_name(s))),
        Value::Str(_) => Ok(v),
        _ => Err(LispError::wrong_type(
            heap,
            "name",
            "symbol, keyword, or string",
            v,
        )),
    }
}

/// `(symbol x)` — the symbol whose spelling is `x`. Accepts a string (intern as
/// a fresh-or-existing symbol), a symbol (identity), or a keyword (same spelling,
/// retagged as a symbol). The lenient inverse of `name`; pairs with `keyword`.
fn to_symbol(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(_) => Ok(v),
        Value::Keyword(s) => Ok(Value::Sym(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::Sym(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "symbol",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(keyword x)` — the keyword whose spelling is `x`. Accepts a string (intern),
/// a keyword (identity), or a symbol (same spelling, retagged as a keyword).
/// Mirrors `symbol`; the two share an interner so a keyword and a symbol with the
/// same spelling carry equal `Symbol` ids (the tag is the only distinction).
fn to_keyword(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Keyword(_) => Ok(v),
        Value::Sym(s) => Ok(Value::Keyword(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::Keyword(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "keyword",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(substring s start [end])` — the characters of `s` in `[start, end)`,
/// char-indexed (consistent with `string-length`). `end` defaults to the
/// string's length, so `(substring s start)` is "from `start` to the end".
/// Errors if out of range.
fn substring(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "substring", arg(args, 0))?;
    let start = expect_int(heap, "substring", arg(args, 1))?;
    let len = s.chars().count() as i64;
    let end = match args.get(2) {
        Some(_) => expect_int(heap, "substring", arg(args, 2))?,
        None => len,
    };
    if start < 0 || end < start || end > len {
        return Err(LispError::runtime(format!(
            "substring: range [{}, {}) out of bounds for length {}",
            start, end, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let sub: String = s
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect();
    Ok(heap.alloc_string(&sub))
}

/// Shared body of `string-span` / `string-span-until`: from char `start`, count the
/// maximal run of chars whose membership in the set `chars` equals `in_set`, and
/// return the char index just past it. Char-indexed, like `substring`/`char-at`. The
/// forward char-class scan a tokenizer runs its inner loops on (skip a whitespace /
/// digit / delimiter run) — O(run) native instead of O(run) interpreted recursion.
fn string_span_impl(args: &[Value], heap: &mut Heap, who: &str, in_set: bool) -> LispResult {
    let s = expect_string(heap, who, arg(args, 0))?;
    let start = expect_int(heap, who, arg(args, 1))?;
    let set = expect_string(heap, who, arg(args, 2))?;
    let len = s.chars().count() as i64;
    if start < 0 || start > len {
        return Err(LispError::runtime(format!(
            "{}: start {} out of bounds for length {}",
            who, start, len
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let mut idx = start as usize;
    for c in s.chars().skip(start as usize) {
        if set.contains(c) == in_set {
            idx += 1;
        } else {
            break;
        }
    }
    Ok(Value::Int(idx as i64))
}

/// `(string-span s start chars)` — the char index just past the maximal run of chars
/// drawn from the set `chars`, beginning at `start` (so `start` itself when the char
/// there isn't in the set). For skipping a run *of* a class — whitespace, digits.
fn string_span(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string-span", true)
}

/// `(string-span-until s start chars)` — the char index of the first char in the set
/// `chars` at or after `start` (or the length if none): the maximal run of chars
/// *not* in the set. For scanning up to a delimiter — comment-to-newline,
/// atom-to-delimiter, string-body-to-quote.
fn string_span_until(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    string_span_impl(args, heap, "string-span-until", false)
}

/// Lexical category of an atom token (a maximal run of non-delimiter chars), matching
/// `std/editor/highlight`'s `hl--atom-face` shape: a `:`-prefixed or `nil`/`true`/`false`
/// constant is a `keyword`; one that parses as an int/float (like `string->number`) is a
/// `number`; anything else is a plain `symbol`. The head-position special-form vs call
/// distinction is left to the consumer (it needs the surrounding `(`).
fn scan_atom_kind(t: &str) -> &'static str {
    if t.starts_with(':') || t == "nil" || t == "true" || t == "false" {
        "keyword"
    } else if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        "number"
    } else {
        "symbol"
    }
}

/// `(scan-tokens s)` — lexically tokenize Brood source `s` into a vector of
/// `[start end kind text]` tokens (char offsets, end-exclusive; whitespace and commas
/// skipped between tokens). `kind` is `:comment`, `:string`, `:number`, `:keyword`,
/// `:symbol`, `:open`, or `:close`. The lossless token stream a fontifier / structural
/// tool walks — the per-character scanning (a render hot path in interpreted Brood) runs
/// here in Rust, leaving the consumer to apply policy (faces, head-position) over
/// O(tokens), not O(chars). Strings honour `\\` escapes; a comment runs to end-of-line.
fn scan_tokens(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "scan-tokens", arg(args, 0))?;
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let kw = |k: &'static str| Value::Keyword(value::intern(k));
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | ',');
    let is_delim = |c: char| is_ws(c) || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';');
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0usize;
    while i < n {
        if is_ws(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let (end, kind): (usize, &'static str) = match chars[i] {
            ';' => {
                let mut j = i + 1;
                while j < n && chars[j] != '\n' {
                    j += 1;
                }
                (j, "comment")
            }
            '"' => {
                let mut j = i + 1;
                loop {
                    if j >= n {
                        break;
                    }
                    match chars[j] {
                        '\\' => j += 2, // escape: skip the backslash and the next char
                        '"' => {
                            j += 1;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                (j.min(n), "string")
            }
            '(' | '[' | '{' => (start + 1, "open"),
            ')' | ']' | '}' => (start + 1, "close"),
            _ => {
                let mut j = i;
                while j < n && !is_delim(chars[j]) {
                    j += 1;
                }
                let text: String = chars[start..j].iter().collect();
                (j, scan_atom_kind(&text))
            }
        };
        let text: String = chars[start..end].iter().collect();
        let tv = heap.alloc_string(&text);
        let tok = heap.alloc_vector(vec![
            Value::Int(start as i64),
            Value::Int(end as i64),
            kw(kind),
            tv,
        ]);
        out.push(tok);
        i = end;
    }
    Ok(heap.alloc_vector(out))
}

/// Append the run `[lo, hi)` (absolute offsets; `base` is the text's first char) in
/// `face` to `runs`, coalescing into the previous run when the faces are `equal` — the
/// runs partition the line contiguously, so coalescing just extends the last run's end.
fn span_runs_push(
    runs: &mut Vec<(usize, usize, Value)>,
    base: i64,
    lo: i64,
    hi: i64,
    face: Value,
    heap: &Heap,
) {
    if hi <= lo {
        return;
    }
    // `lo`/`hi` are absolute offsets >= `base` by construction; `saturating_sub`
    // keeps the relative index non-negative even if a caller ever violated that,
    // so the host can't panic on an underflow.
    let lhi = hi.saturating_sub(base) as usize;
    if let Some(last) = runs.last_mut() {
        if heap.equal(last.2, face) {
            last.1 = lhi;
            return;
        }
    }
    runs.push((lo.saturating_sub(base) as usize, lhi, face));
}

/// Merge face `b` over face `a` (`b` wins on key conflict), as Brood's `(into a b)` —
/// the overlay-merge the fontifier does to paint a region/isearch face on top of a
/// syntax face. A nil face is the identity; two maps merge `b`'s entries into `a`.
fn merge_faces(heap: &mut Heap, a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Nil, _) => b,
        (_, Value::Nil) => a,
        (Value::Map(ai), Value::Map(bi)) => {
            let entries = heap.map_entries(bi);
            heap.map_from_pairs_into(ai, entries)
        }
        _ => b,
    }
}

/// Read a `[start end face]` span/range list into `(start, end, face)` tuples (handles
/// at offsets outside the window are kept; the tilers clip them).
fn read_spans(heap: &Heap, who: &str, v: Value) -> Result<Vec<(i64, i64, Value)>, LispError> {
    let items = heap.seq_items(v)?;
    let mut out = Vec::with_capacity(items.len());
    for sv in &items {
        let parts = match sv {
            Value::Vector(id) => heap.vector(*id).to_vec(),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be a [start end face] vector",
                    who
                )))
            }
        };
        match (parts.first(), parts.get(1), parts.get(2)) {
            (Some(Value::Int(s)), Some(Value::Int(e)), Some(f)) => out.push((*s, *e, *f)),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be [int int face]",
                    who
                )))
            }
        }
    }
    Ok(out)
}

/// `(span-runs text base spans [ranges])` — tile `text` (its first char at offset
/// `base`) into a list of `[substring face]` runs. From ascending, non-overlapping
/// `[start end face]` `spans`: each gap is a nil-faced run, each span its text in its
/// face. With an optional overlay `ranges` channel (`[lo hi face]`, may overlap /
/// be unordered), each char's face is its span face with every covering range face
/// merged on top (later ranges win) — the region / isearch / bracket overlays. Adjacent
/// equal-face runs coalesce. This is the fontifier's span→runs tiler (`std/editor/
/// highlight`'s `fontify-runs`) in Rust — it runs per visible line every frame. Faces
/// are opaque maps, merged via `into` semantics and compared with `equal` to coalesce.
fn span_runs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let text = expect_string(heap, "span-runs", arg(args, 0))?;
    let base = expect_int(heap, "span-runs", arg(args, 1))?;
    let spans = read_spans(heap, "span-runs", arg(args, 2))?;
    let ranges = match args.get(3) {
        Some(r) => read_spans(heap, "span-runs", *r)?,
        None => Vec::new(),
    };
    let chars: Vec<char> = text.chars().collect();
    // `base` is caller-controlled (any i64); guard the absolute end against i64
    // overflow so a Lisp program can't panic the host. With a valid `end`, every
    // `lo`/`hi` handed to `span_runs_push` is provably in `[base, end]`.
    let end = base.checked_add(chars.len() as i64).ok_or_else(|| {
        LispError::runtime(format!(
            "span-runs: base {base} plus text length {} overflows i64",
            chars.len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
    })?;
    let mut runs: Vec<(usize, usize, Value)> = Vec::new();

    if ranges.is_empty() {
        // fast path: no overlay merge — emit gaps + spans left-to-right.
        let mut cur = base;
        for (s, e, f) in spans {
            if e <= base {
                continue;
            }
            if s >= end {
                break; // ascending spans: the rest are past the window
            }
            let lo = s.max(cur);
            let hi = e.min(end);
            if lo > cur {
                span_runs_push(&mut runs, base, cur, lo, Value::Nil, heap);
            }
            span_runs_push(&mut runs, base, lo, hi, f, heap);
            cur = hi;
        }
        if cur < end {
            span_runs_push(&mut runs, base, cur, end, Value::Nil, heap);
        }
    } else {
        // overlay path: tile by the union of span + range edges, merging faces per
        // segment. O(segments) — segments, not chars — so a region over the viewport is
        // as cheap as plain syntax, not a per-character merge.
        let mut bounds: Vec<i64> = vec![base, end];
        for (s, e, _) in spans.iter().chain(ranges.iter()) {
            if *e > base && *s < end {
                bounds.push((*s).max(base));
                bounds.push((*e).min(end));
            }
        }
        bounds.sort_unstable();
        bounds.dedup();
        let mut si = 0usize; // monotonic span cursor (spans are ascending)
        for w in bounds.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a {
                continue;
            }
            while si < spans.len() && spans[si].1 <= a {
                si += 1;
            }
            let span_face = if si < spans.len() && spans[si].0 <= a && a < spans[si].1 {
                spans[si].2
            } else {
                Value::Nil
            };
            let mut rf = Value::Nil;
            for (lo, hi, f) in &ranges {
                if *lo <= a && a < *hi {
                    rf = merge_faces(heap, rf, *f);
                }
            }
            let face = merge_faces(heap, span_face, rf);
            span_runs_push(&mut runs, base, a, b, face, heap);
        }
    }

    let n = chars.len();
    let out: Vec<Value> = runs
        .iter()
        .map(|&(lo, hi, f)| {
            // Clamp defensively: the run bounds are in-range by construction, but a
            // slice past `chars.len()` would panic the host — never let it.
            let seg: String = chars[lo.min(n)..hi.min(n)].iter().collect();
            let sv = heap.alloc_string(&seg);
            heap.alloc_vector(vec![sv, f])
        })
        .collect();
    Ok(heap.list_from_slice(&out))
}

/// OS clipboard access (the `clipboard` feature, via `arboard`). The handle lives in a
/// `OnceLock` for the whole process: on X11/Wayland the selection *owner* must stay
/// alive to answer paste requests, so a fresh handle per call would lose the copied text
/// the moment it dropped. Init failure (no display server) is cached as `None`, so the
/// builtins degrade to no-ops rather than retrying.
#[cfg(feature = "clipboard")]
mod clipboard {
    use arboard::Clipboard;
    use std::sync::{Mutex, OnceLock};
    static CB: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();
    fn handle() -> Option<&'static Mutex<Clipboard>> {
        CB.get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }
    pub fn get_text() -> Option<String> {
        handle()?.lock().ok()?.get_text().ok()
    }
    pub fn set_text(s: &str) {
        if let Some(m) = handle() {
            if let Ok(mut cb) = m.lock() {
                let _ = cb.set_text(s.to_owned());
            }
        }
    }
}

/// `(clipboard-get)` — the OS clipboard's text, or nil when it's empty / non-text /
/// unavailable (no display server, or a build without the `clipboard` feature). The
/// editor's yank consults this so text copied in another app pastes in.
fn clipboard_get(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    #[cfg(feature = "clipboard")]
    if let Some(s) = clipboard::get_text() {
        return Ok(heap.alloc_string(&s));
    }
    #[cfg(not(feature = "clipboard"))]
    let _ = &heap;
    Ok(Value::Nil)
}

/// `(clipboard-set! s)` — copy string `s` to the OS clipboard so other apps can paste
/// it; returns `s` (so it threads). A no-op (still returns `s`) when no clipboard is
/// available or the `clipboard` feature is off, so callers needn't special-case headless
/// builds. The editor's kill/copy commands call this so a kill is system-wide.
fn clipboard_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "clipboard-set!", arg(args, 0))?;
    #[cfg(feature = "clipboard")]
    clipboard::set_text(&s);
    #[cfg(not(feature = "clipboard"))]
    let _ = &s;
    Ok(arg(args, 0))
}

/// `(%str-index-of s needle)` — the 0-based **char** index of the first
/// occurrence of `needle` in `s`, or -1 if absent. Linear: Rust's byte-level
/// `str::find`, then a one-pass byte→char-index conversion of the prefix. The
/// empty needle matches at 0 (matching `index-of`'s contract). The search
/// primitive the Brood `index-of`/`string-contains?` ride on; see the
/// registration comment for why this can't be efficient in pure Brood.
fn str_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%str-index-of", arg(args, 0))?;
    let needle = expect_string(heap, "%str-index-of", arg(args, 1))?;
    let idx = match s.find(needle.as_str()) {
        Some(byte) => s[..byte].chars().count() as i64,
        None => -1,
    };
    Ok(Value::Int(idx))
}

/// `(string-split s sep)` — split `s` into a list of substrings on each occurrence
/// of `sep`, in one O(n) pass. An empty separator splits `s` into its individual
/// characters (1-char strings). Mirrors the semantics of the former pure-Brood
/// `string-split`/`string->list`, but without the O(n²) tail-substring rebuild.
fn string_split(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string-split", arg(args, 0))?;
    let sep = expect_string(heap, "string-split", arg(args, 1))?;
    let out: Vec<Value> = if sep.is_empty() {
        s.chars().map(|c| heap.alloc_string(&c.to_string())).collect()
    } else {
        s.split(sep.as_str()).map(|part| heap.alloc_string(part)).collect()
    };
    Ok(heap.list_from_slice(&out))
}

/// `(to-fixed x n)` — x rendered with exactly `n` digits after the decimal point
/// (rounded). The one float→text op the language can't bootstrap: `str`/`pr-str`
/// print the shortest round-tripping form (full f64 precision, e.g.
/// `0.015873015873015872`), which is wrong for tabular/console output. An int `x`
/// is promoted, so `(to-fixed 3 2)` is `"3.00"`. `n` must be non-negative.
fn to_fixed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = expect_number(heap, "to-fixed", arg(args, 0))?;
    let n = expect_int(heap, "to-fixed", arg(args, 1))?;
    if n < 0 {
        return Err(LispError::runtime(format!(
            "to-fixed: decimal places must be non-negative, got {}",
            n
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    // Bound the width: `format!("{:.*}", n, x)` materialises an `n`-digit string,
    // so an unbounded `n` (e.g. `(to-fixed 1.0 1000000000)`) allocates ~1 GB on the
    // Rust side, bypassing the GC/soft-memory cap. An f64 carries ~17 significant
    // digits; past that the tail is just zeros, so 1000 is far beyond any real use
    // while keeping the worst-case alloc to ~1 KB.
    const MAX_DECIMALS: i64 = 1000;
    if n > MAX_DECIMALS {
        return Err(LispError::runtime(format!(
            "to-fixed: decimal places {n} too large (max {MAX_DECIMALS}); an f64 has \
             ~17 significant digits, so a larger count only pads zeros"
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE));
    }
    let s = format!("{:.*}", n as usize, x);
    Ok(heap.alloc_string(&s))
}

/// `(upper s)` — `s` with every character upper-cased. Case folding is
/// Unicode-aware (e.g. `ß` → `SS`), so it leans on the standard library's tables
/// rather than being expressible in Brood.
fn upper(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "upper", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_uppercase()))
}

/// `(lower s)` — `s` with every character lower-cased (Unicode-aware, like `upper`).
fn lower(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "lower", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_lowercase()))
}

fn char_to_int(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "char->int", arg(args, 0))?;
    match s.chars().next() {
        Some(c) => Ok(Value::Int(c as i64)),
        None => Err(LispError::runtime("char->int: empty string")),
    }
}

fn int_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "int->char", arg(args, 0))?;
    let c = char::from_u32(n as u32).ok_or_else(|| {
        LispError::runtime(format!("int->char: {} is not a valid Unicode codepoint", n))
    })?;
    let mut buf = [0u8; 4];
    Ok(heap.alloc_string(c.encode_utf8(&mut buf)))
}

fn string_to_utf8_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->utf8-bytes", arg(args, 0))?;
    let items: Vec<Value> = s.as_bytes().iter().map(|&b| Value::Int(b as i64)).collect();
    Ok(heap.alloc_vector(items))
}

fn utf8_bytes_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Accepts a vector *or* a proper list of byte integers (0–255).
    let v = arg(args, 0);
    let items: Vec<Value> = match v {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil => vec![],
        Value::Pair(_) => {
            let mut out = Vec::new();
            let mut cur = v;
            loop {
                match cur {
                    Value::Pair(id) => {
                        let (head, tail) = heap.pair(id);
                        out.push(head);
                        cur = tail;
                    }
                    Value::Nil => break,
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "utf8-bytes->string",
                            "proper list",
                            other,
                        ))
                    }
                }
            }
            out
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "utf8-bytes->string",
                "vector or list",
                other,
            ))
        }
    };
    let mut bytes = Vec::with_capacity(items.len());
    for (i, val) in items.iter().enumerate() {
        match val {
            Value::Int(n) if *n >= 0 && *n <= 255 => bytes.push(*n as u8),
            Value::Int(n) => {
                return Err(LispError::runtime(format!(
                    "utf8-bytes->string: byte at index {} is out of range: {}",
                    i, n
                )))
            }
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "utf8-bytes->string",
                    "int",
                    *other,
                ))
            }
        }
    }
    match String::from_utf8(bytes) {
        Ok(s) => Ok(heap.alloc_string(&s)),
        Err(e) => Err(LispError::runtime(format!(
            "utf8-bytes->string: invalid UTF-8: {}",
            e
        ))),
    }
}

// ---------- transcendental math ----------

macro_rules! math1_unrestricted {
    ($name:ident, $brood:literal, $method:ident) => {
        fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = expect_number(heap, $brood, arg(args, 0))?;
            Ok(Value::Float(x.$method()))
        }
    };
}

macro_rules! math1_bounded {
    ($name:ident, $brood:literal, $method:ident) => {
        fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = expect_number(heap, $brood, arg(args, 0))?;
            if x < -1.0 || x > 1.0 {
                return Err(LispError::runtime(format!(
                    "{}: argument {} is out of domain [-1, 1]",
                    $brood, x
                )));
            }
            Ok(Value::Float(x.$method()))
        }
    };
}

macro_rules! math1_positive {
    ($name:ident, $brood:literal, $method:ident) => {
        fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = expect_number(heap, $brood, arg(args, 0))?;
            if x <= 0.0 {
                return Err(LispError::runtime(format!(
                    "{}: argument {} must be positive",
                    $brood, x
                )));
            }
            Ok(Value::Float(x.$method()))
        }
    };
}

math1_unrestricted!(math_sin, "sin", sin);
math1_unrestricted!(math_cos, "cos", cos);
math1_unrestricted!(math_tan, "tan", tan);
math1_unrestricted!(math_atan, "atan", atan);
math1_unrestricted!(math_exp, "exp", exp);
math1_bounded!(math_asin, "asin", asin);
math1_bounded!(math_acos, "acos", acos);
math1_positive!(math_ln, "ln", ln);
math1_positive!(math_log2, "log2", log2);
math1_positive!(math_log10, "log10", log10);

fn math_atan2(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let y = expect_number(heap, "atan2", arg(args, 0))?;
    let x = expect_number(heap, "atan2", arg(args, 1))?;
    Ok(Value::Float(y.atan2(x)))
}

// ---------- rope (editor buffer text — ADR-045) ----------
//
// All indices are **character** indices (matching the language's char-based
// string indexing), not bytes. Edits return a *fresh* rope (immutability):
// ropey clones share structure, so `clone()`-then-edit only copies touched
// B-tree nodes. Out-of-range indices raise a clean E-code error rather than
// letting ropey panic.

/// Raise a uniform out-of-range error attributed to `who`.
fn rope_oob(who: &str, what: &str, got: i64, max: usize) -> LispError {
    LispError::runtime(format!(
        "{}: {} {} out of bounds (valid 0..={})",
        who, what, got, max
    ))
    .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
}

/// `(string->rope s)` — a rope holding the text of string `s`.
fn string_to_rope(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->rope", arg(args, 0))?;
    Ok(heap.alloc_rope(ropey::Rope::from_str(&s)))
}

/// `(rope->string r)` — the full text of rope `r` as a string.
fn rope_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope->string", arg(args, 0))?;
    Ok(heap.alloc_string(&r.to_string()))
}

/// `(rope-length r)` — the number of characters in `r`.
fn rope_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-length", arg(args, 0))?;
    Ok(Value::Int(r.len_chars() as i64))
}

/// `(rope-line-count r)` — the number of lines in `r` (ropey counts a trailing
/// newline as ending a line, so `"a\n"` is 2 lines and `""` is 1).
fn rope_line_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line-count", arg(args, 0))?;
    Ok(Value::Int(r.len_lines() as i64))
}

/// `(rope-insert r idx s)` — a fresh rope with string `s` inserted at character
/// index `idx` (0..=length).
fn rope_insert(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "rope-insert", arg(args, 0))?;
    let idx = expect_int(heap, "rope-insert", arg(args, 1))?;
    let s = expect_string(heap, "rope-insert", arg(args, 2))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("rope-insert", "index", idx, len));
    }
    r.insert(idx as usize, &s);
    Ok(heap.alloc_rope(r))
}

/// `(rope-delete r start end)` — a fresh rope with characters `[start, end)`
/// removed.
fn rope_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "rope-delete", arg(args, 0))?;
    let start = expect_int(heap, "rope-delete", arg(args, 1))?;
    let end = expect_int(heap, "rope-delete", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("rope-delete", "range end", end, len));
    }
    r.remove(start as usize..end as usize);
    Ok(heap.alloc_rope(r))
}

/// `(rope-slice r start end)` — the text of characters `[start, end)` as a string.
fn rope_slice(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-slice", arg(args, 0))?;
    let start = expect_int(heap, "rope-slice", arg(args, 1))?;
    let end = expect_int(heap, "rope-slice", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("rope-slice", "range end", end, len));
    }
    let s = r.slice(start as usize..end as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-line r n)` — the text of line `n` (0-based), including its trailing
/// newline if present. The viewport-rendering primitive.
fn rope_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line", arg(args, 0))?;
    let n = expect_int(heap, "rope-line", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize >= lines {
        return Err(rope_oob("rope-line", "line", n, lines.saturating_sub(1)));
    }
    let s = r.line(n as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-char->line r idx)` — the 0-based line index containing character `idx`.
fn rope_char_to_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-char->line", arg(args, 0))?;
    let idx = expect_int(heap, "rope-char->line", arg(args, 1))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("rope-char->line", "index", idx, len));
    }
    Ok(Value::Int(r.char_to_line(idx as usize) as i64))
}

/// `(rope-line->char r n)` — the character index where line `n` (0-based) begins.
fn rope_line_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope(heap, "rope-line->char", arg(args, 0))?;
    let n = expect_int(heap, "rope-line->char", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize > lines {
        return Err(rope_oob("rope-line->char", "line", n, lines));
    }
    Ok(Value::Int(r.line_to_char(n as usize) as i64))
}

// ---------- TCP sockets (ADR-062) ----------
//
// Thin non-blocking mechanism over `crate::net`; the active-socket / framing /
// HTTP policy is Brood (std/tcp.blsp). A socket is `Value::Socket(id)`.

fn expect_socket(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "socket",
        Value::Socket(id) => id,
    )
}

// ---------- in-memory shared table (Brood's ETS, ADR-107) ----------
// A `Value::Table(id)` handle; the store lives in `crate::table`. These builtins are
// thin wrappers — all the storage / locking / clone-in-clone-out lives there.

fn expect_table(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "table",
        Value::Table(id) => id,
    )
}

fn table_new(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Table(crate::table::create()))
}

fn table_put(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-put", arg(args, 0))?;
    crate::table::check_key("table-put", arg(args, 1))?;
    crate::table::put(heap, id, arg(args, 1), arg(args, 2))
}

fn table_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-get", arg(args, 0))?;
    crate::table::check_key("table-get", arg(args, 1))?;
    crate::table::get(heap, id, arg(args, 1), arg(args, 2))
}

fn table_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-has?", arg(args, 0))?;
    crate::table::check_key("table-has?", arg(args, 1))?;
    Ok(Value::Bool(crate::table::has(heap, id, arg(args, 1))?))
}

fn table_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-delete", arg(args, 0))?;
    crate::table::check_key("table-delete", arg(args, 1))?;
    crate::table::delete(heap, id, arg(args, 1))
}

fn table_incr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-incr", arg(args, 0))?;
    crate::table::check_key("table-incr", arg(args, 1))?;
    let delta = match arg(args, 2) {
        Value::Nil => 1, // (table-incr t k) defaults the delta to 1
        v => expect_int(heap, "table-incr", v)?,
    };
    crate::table::incr(heap, id, arg(args, 1), delta)
}

fn table_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-count", arg(args, 0))?;
    Ok(Value::Int(crate::table::count(id)?))
}

fn table_snapshot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-snapshot", arg(args, 0))?;
    crate::table::snapshot(heap, id)
}

fn table_drop(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-drop", arg(args, 0))?;
    Ok(Value::Bool(crate::table::drop_table(id)))
}

fn socket_port(who: &str, p: i64) -> Result<u16, LispError> {
    u16::try_from(p)
        .map_err(|_| LispError::runtime(format!("{}: port {} out of range 0..=65535", who, p)))
}

fn tcp_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tcp-connect", arg(args, 0))?;
    let port = socket_port(
        "tcp-connect",
        expect_int(heap, "tcp-connect", arg(args, 1))?,
    )?;
    let owner = crate::process::self_pid();
    match crate::net::connect(&host, port, owner) {
        Ok(id) => Ok(Value::Socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tcp-connect {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

fn tcp_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tcp-listen", arg(args, 0))?;
    let port = socket_port("tcp-listen", expect_int(heap, "tcp-listen", arg(args, 1))?)?;
    let owner = crate::process::self_pid();
    match crate::net::listen(&host, port, owner) {
        Ok(id) => Ok(Value::Socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tcp-listen {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

fn tls_request(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tls-request", arg(args, 0))?;
    let port = socket_port(
        "tls-request",
        expect_int(heap, "tls-request", arg(args, 1))?,
    )?;
    let request = expect_string(heap, "tls-request", arg(args, 2))?;
    let owner = crate::process::self_pid();
    let id = crate::net::tls_request(&host, port, request.to_string(), owner);
    Ok(Value::Socket(id))
}

fn tcp_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-send", arg(args, 0))?;
    let data = expect_string(heap, "tcp-send", arg(args, 1))?;
    if crate::net::is_binary(id) {
        // Binary mode: write each codepoint as one raw byte (Latin-1). The string
        // must be a byte-string (codepoints 0–255) — e.g. a WebSocket frame the
        // caller built by UTF-8-encoding any text payload into this form.
        let mut out = Vec::with_capacity(data.len());
        for c in data.chars() {
            let n = c as u32;
            if n > 0xFF {
                return Err(LispError::runtime(format!(
                    "tcp-send: codepoint U+{:04X} is not a byte (0–255); a binary-mode socket sends raw bytes only",
                    n
                )));
            }
            out.push(n as u8);
        }
        crate::net::send(id, &out)
    } else {
        crate::net::send(id, data.as_bytes())
    }
    .map_err(|e| LispError::runtime(format!("tcp-send: {}", e)))?;
    Ok(Value::Nil)
}

fn tcp_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-set-binary", arg(args, 0))?;
    let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
    crate::net::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("tcp-set-binary: {}", e)))?;
    Ok(Value::Nil)
}

fn tcp_controlling_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-controlling-process", arg(args, 0))?;
    let pid = match arg(args, 1) {
        Value::Pid { id, .. } => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "tcp-controlling-process",
                "pid",
                other,
            ))
        }
    };
    crate::net::controlling_process(id, pid)
        .map_err(|e| LispError::runtime(format!("tcp-controlling-process: {}", e)))?;
    Ok(Value::Nil)
}

fn tcp_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-close", arg(args, 0))?;
    crate::net::close(id);
    Ok(Value::Nil)
}

fn tcp_local_port(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-local-port", arg(args, 0))?;
    Ok(crate::net::local_port(id)
        .map(|p| Value::Int(p as i64))
        .unwrap_or(Value::Nil))
}

// ----- persistent child processes (ADR-104) ----------------------------------
//
// Thin mechanism over `crate::proc`: spawn a long-lived child with piped stdio,
// write its stdin, and receive its output as `[:proc …]` mailbox messages. The
// framing/protocol policy (e.g. JSON-RPC for an LSP client) is Brood. A child is
// `Value::Subprocess(id)`. Contrast `%os-cmd`/`run-process`, which run to exit.

fn expect_subprocess(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "subprocess",
        Value::Subprocess(id) => id,
    )
}

fn proc_spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "proc-spawn", arg(args, 0))?;
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        argv.push(expect_string(heap, "proc-spawn", a)?);
    }
    // Optional 3rd argument: an options map `{:cwd "dir" :env {"K" "V" …}}`.
    let mut cwd: Option<String> = None;
    let mut env: Vec<(String, String)> = Vec::new();
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(v) = heap.map_get(opts, Value::Keyword(value::intern("cwd"))) {
            if !matches!(v, Value::Nil) {
                cwd = Some(expect_string(heap, "proc-spawn :cwd", v)?);
            }
        }
        if let Some(Value::Map(e)) = heap.map_get(opts, Value::Keyword(value::intern("env"))) {
            for (k, v) in heap.map_entries(e) {
                env.push((
                    expect_string(heap, "proc-spawn :env key", k)?,
                    expect_string(heap, "proc-spawn :env value", v)?,
                ));
            }
        }
    }
    let owner = crate::process::self_pid();
    match crate::proc::spawn(&prog, &argv, cwd.as_deref(), &env, owner) {
        Ok(id) => Ok(Value::Subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("proc-spawn {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

fn proc_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-send", arg(args, 0))?;
    let data = expect_string(heap, "proc-send", arg(args, 1))?;
    crate::proc::send(id, data.as_bytes())
        .map_err(|e| LispError::runtime(format!("proc-send: {}", e)))?;
    Ok(Value::Nil)
}

fn proc_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-close", arg(args, 0))?;
    crate::proc::close(id);
    Ok(Value::Nil)
}

// ----- terminal frontend (ADR-046) -------------------------------------------
//
// The thin crossterm seam: enter/leave the alternate screen, read keys, and
// paint a *frame* — a Brood vector of render ops. The protocol's meaning is
// data (the ops); these primitives are the in-process frontend that interprets
// it, so a remote/web frontend can implement the identical op vocabulary later.
// Errors surface as clean `LispError`s (never a crossterm panic), mirroring the
// rope primitives' discipline.

/// Map a crossterm I/O error into a runtime `LispError`.
fn term_err(e: std::io::Error) -> LispError {
    LispError::runtime(format!("terminal: {}", e))
}

/// `(term-enter)` — take over the terminal: raw mode + alternate screen, cursor
/// hidden. Pair with `term-leave`. The Rust-side `nest observe` guard also
/// restores the terminal if the program panics, so a crash never wrecks it.
fn term_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Hide;
    use crossterm::event::EnableMouseCapture;
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
    enable_raw_mode().map_err(term_err)?;
    // Mouse capture rides with the full-screen path only (not the inline REPL
    // seam), so click/scroll reach `term-poll`. It costs terminal text-selection
    // while active — standard for a TUI, and only for the duration of the UI.
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        Hide
    )
    .map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-leave)` — restore the terminal (show cursor, leave alternate screen,
/// disable raw mode). The normal-path teardown for `term-enter`.
fn term_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .map_err(term_err)?;
    disable_raw_mode().map_err(term_err)?;
    Ok(Value::Nil)
}

/// Best-effort terminal restore — the abnormal-path backstop a host binary holds
/// in an RAII guard so a panic or error during a full-screen UI (`nest observe`)
/// never leaves the terminal in raw mode / the alternate screen. Idempotent and
/// errors are swallowed (the normal path is the Brood `term-leave`).
pub fn restore_terminal() {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    let _ = crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// Lighter restore for the *inline* seam (the REPL line editor, `term-raw-enter`):
/// only leave raw mode. Unlike `restore_terminal` it writes no escape sequences —
/// `disable_raw_mode` is a termios ioctl — so a host binary can hold this in an
/// RAII guard around the REPL without polluting a piped (non-TTY) stdout on exit.
/// Idempotent; errors are swallowed (the normal path is the Brood `term-raw-leave`).
pub fn restore_raw() {
    let _ = crossterm::terminal::disable_raw_mode();
}

/// The terminal restore the binaries call on **every** exit path — normal
/// return, error report, and the broken-pipe exit in `print`. A program that
/// entered raw mode / the alternate screen (`term-raw-enter` / `term-enter`)
/// and then threw — or simply returned without a matching `term-raw-leave` —
/// would otherwise leave the shell wedged in raw mode (the hung-terminal bug).
///
/// It is gated on `is_raw_mode_enabled`, so it is a precise no-op whenever the
/// terminal was never left raw: that lets it sit on the common path (e.g. a
/// `nest test` run that never touched the terminal) without emitting a single
/// stray escape. When a restore *is* needed: on a TTY it does the full
/// [`restore_terminal`] (show cursor, leave the alternate screen and raw mode);
/// when stdout is piped/redirected it only leaves raw mode ([`restore_raw`], a
/// termios ioctl) so it never writes escape bytes into a captured/closed
/// stream. Idempotent.
pub fn restore_terminal_on_exit() {
    // Only act if the program actually left the terminal in raw mode. This is
    // what makes the call safe to drop onto the success path too, not just the
    // error/broken-pipe paths.
    if !matches!(crossterm::terminal::is_raw_mode_enabled(), Ok(true)) {
        return;
    }
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        restore_terminal();
    } else {
        restore_raw();
    }
}

/// `(term-size)` — the terminal size as `[cols rows]` (character cells).
fn term_size(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (cols, rows) = crossterm::terminal::size().map_err(term_err)?;
    Ok(heap.alloc_vector(vec![Value::Int(cols as i64), Value::Int(rows as i64)]))
}

/// `(term-poll ms)` — wait up to `ms` ms for a key; return it (a 1-char string,
/// or a keyword for specials) or `nil` on timeout. Always called with a finite
/// `ms`: the observer is the root process, so blocking here blocks only the root
/// thread (never a scheduler worker), but an *infinite* poll on a green process
/// would pin a worker (native blocking can't be preempted) — hence finite.
fn term_poll(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::event::{poll, read, Event, KeyEventKind};
    let ms = expect_int(heap, "term-poll", arg(args, 0))?.max(0) as u64;
    if poll(std::time::Duration::from_millis(ms)).map_err(term_err)? {
        match read().map_err(term_err)? {
            // Ignore key *release* events (reported on some platforms with the
            // enhanced-keyboard protocol) so a keypress isn't seen twice.
            Event::Key(k) if k.kind != KeyEventKind::Release => Ok(key_to_value(heap, k)),
            Event::Mouse(m) => Ok(mouse_to_value(heap, m)),
            _ => Ok(Value::Nil),
        }
    } else {
        Ok(Value::Nil)
    }
}

/// Encode a Brood mouse event as the vector `[:mouse action button row col mods]`
/// — the shared shape both frontends yield, so the observer (and any future UI)
/// reads one form. `action` is a keyword, `button` a keyword or nil, `row`/`col`
/// 0-based cell coordinates, `mods` a vector of the held modifier keywords (in a
/// stable `[:ctrl :alt :shift]` order, `[]` when none) so an app can bind
/// Ctrl+wheel etc.
fn mouse_value(
    heap: &mut Heap,
    action: &str,
    button: Option<&str>,
    row: u16,
    col: u16,
    mods: (bool, bool, bool),
    count: u8,
) -> Value {
    let btn = button.map(value::kw).unwrap_or(Value::Nil);
    let (ctrl, alt, shift) = mods;
    let mut ms = Vec::new();
    if ctrl {
        ms.push(value::kw("ctrl"));
    }
    if alt {
        ms.push(value::kw("alt"));
    }
    if shift {
        ms.push(value::kw("shift"));
    }
    let ms = heap.alloc_vector(ms);
    let mut v = vec![
        value::kw("mouse"),
        value::kw(action),
        btn,
        Value::Int(row as i64),
        Value::Int(col as i64),
        ms,
    ];
    // A press carries its click-chain count as a trailing 7th element; other actions
    // (count 0) stay 6-element. The terminal can't detect multi-click, so it reports 1
    // for every press — keeping the GUI and terminal shapes identical.
    if count > 0 {
        v.push(Value::Int(count as i64));
    }
    heap.alloc_vector(v)
}

/// Translate a crossterm mouse event into the shared `[:mouse …]` vector.
/// Press, release, drag, and vertical scroll are surfaced (the `:release`/`:drag`
/// vocabulary `gui::MouseAction` also produces per ADR-077). Only bare `Moved`
/// (motion with no button held) and horizontal scroll fall through to nil (a
/// no-op poll), so both frontends emit exactly the same set.
fn mouse_to_value(heap: &mut Heap, m: crossterm::event::MouseEvent) -> Value {
    use crossterm::event::{KeyModifiers, MouseButton as CB, MouseEventKind as MK};
    let button = |b: CB| match b {
        CB::Left => "left",
        CB::Right => "right",
        CB::Middle => "middle",
    };
    let (action, btn, count) = match m.kind {
        // The terminal reports no click chain, so a press always counts as 1 (single);
        // the trailing count keeps the terminal vector shape identical to the GUI's.
        MK::Down(b) => ("press", Some(button(b)), 1),
        MK::Up(b) => ("release", Some(button(b)), 0),
        // Motion with a button held — a drag (e.g. resizing a divider, ADR-077).
        // Crossterm already reports this per-cell, matching the GUI's cell-granular
        // throttle. Bare `Moved` (no button) falls through to nil, as before.
        MK::Drag(b) => ("drag", Some(button(b)), 0),
        MK::ScrollUp => ("scroll-up", None, 0),
        MK::ScrollDown => ("scroll-down", None, 0),
        _ => return Value::Nil,
    };
    let mods = (
        m.modifiers.contains(KeyModifiers::CONTROL),
        m.modifiers.contains(KeyModifiers::ALT),
        m.modifiers.contains(KeyModifiers::SHIFT),
    );
    mouse_value(heap, action, btn, m.row, m.column, mods, count)
}

/// Encode a crossterm key event as a Brood value: a printable char becomes a
/// 1-char string; a control combo and the named special keys become keywords.
fn key_to_value(heap: &mut Heap, k: crossterm::event::KeyEvent) -> Value {
    use crossterm::event::{KeyCode, KeyModifiers};
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        // Ctrl+Alt (Emacs C-M-… — structural sexp motion C-M-f/b/u/d, mark-sexp
        // C-M-SPC). Must precede the ctrl-only / alt-only arms so the second modifier
        // isn't dropped (the `ctrl-meta-` spelling the keymaps bind).
        KeyCode::Char(c) if ctrl && alt => Value::Keyword(value::intern(&format!(
            "ctrl-meta-{}",
            c.to_ascii_lowercase()
        ))),
        KeyCode::Char(c) if ctrl => {
            Value::Keyword(value::intern(&format!("ctrl-{}", c.to_ascii_lowercase())))
        }
        // Alt/Meta combos (M-f, M-b, … — emacs word motion). Some terminals send
        // these as an Esc prefix; crossterm normalises them to the ALT modifier.
        KeyCode::Char(c) if alt => {
            // Meta is case-SENSITIVE (`M-O` ≠ `M-o`): keep a shifted letter upper-case so
            // the two are distinct; an unshifted chord lower-cases (Caps Lock / a stray
            // Shift can't change the binding). Mirrors the GUI frontend
            // (`gui::backend::translate_key`); Control chords above stay case-insensitive.
            let ch = if k.modifiers.contains(KeyModifiers::SHIFT) {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            Value::Keyword(value::intern(&format!("alt-{ch}")))
        }
        KeyCode::Char(c) => heap.alloc_string(&c.to_string()),
        KeyCode::Up => value::kw("up"),
        KeyCode::Down => value::kw("down"),
        KeyCode::Left => value::kw("left"),
        KeyCode::Right => value::kw("right"),
        KeyCode::Enter => value::kw("enter"),
        KeyCode::Esc => value::kw("escape"),
        KeyCode::Backspace => value::kw("backspace"),
        KeyCode::Tab => value::kw("tab"),
        KeyCode::BackTab => value::kw("back-tab"),
        KeyCode::Delete => value::kw("delete"),
        KeyCode::Home => value::kw("home"),
        KeyCode::End => value::kw("end"),
        KeyCode::PageUp => value::kw("page-up"),
        KeyCode::PageDown => value::kw("page-down"),
        _ => Value::Nil,
    }
}

/// `(term-draw frame)` — paint a frame: a vector of op vectors `[:clear]`,
/// `[:text row col str]`, `[:text row col str face]`, `[:cursor row col]`.
/// Unknown ops are skipped (forward-compatible protocol). Queues all ops then
/// flushes once, so a frame paints without intermediate tearing.
/// Write rendered terminal bytes (escape sequences) to stdout — unless an MCP
/// stdout-capture is active on this thread, in which case divert them into the
/// capture buffer instead. During a `nest mcp` `tools/call`, stdout *is* the
/// JSON-RPC channel, so a `term-draw` / `term-emit` writing raw escapes there would
/// corrupt the protocol and wedge the client (the `print` capture only catches
/// Brood `print`, not these direct crossterm writes). Diverting keeps the channel
/// pure and rides the rendered bytes back in the result envelope, so an agent can
/// still inspect what a frame produced. Mirrors `print`'s capture check.
fn write_term_bytes(bytes: &[u8]) -> std::io::Result<()> {
    if !capture_write(&String::from_utf8_lossy(bytes)) {
        use std::io::Write;
        let mut real = std::io::stdout();
        real.write_all(bytes)?;
        real.flush()?;
    }
    Ok(())
}

/// Parse a frame value (the op-vector `term-draw`/`term-emit`/`gui-draw` all
/// take) into `(tag, parts)` pairs: the frame must be a `Vector` (else a
/// `wrong_type` attributed to `who`); each op that is itself a `Vector` whose
/// first element is a `Keyword` yields `(that-keyword, the-op-parts)`; any op
/// that isn't a keyword-led vector is silently skipped (forward-compatible —
/// unknown ops are no-ops). This is the one extraction shared verbatim by the
/// three frame dispatchers; they deliberately *diverge downstream* (e.g. gui-draw
/// clamps coords at parse time, term-draw at use), so they must not drift on this
/// shared prologue — keep it here, in one place.
fn frame_ops(
    heap: &Heap,
    frame: Value,
    who: &str,
    expected: &str,
) -> Result<Vec<(value::Symbol, Vec<Value>)>, LispError> {
    let ops: Vec<Value> = match frame {
        Value::Vector(id) => heap.vector(id).to_vec(),
        other => return Err(LispError::wrong_type(heap, who, expected, other)),
    };
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        let parts: Vec<Value> = match op {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => continue,
        };
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => continue,
        };
        out.push((tag, parts));
    }
    Ok(out)
}

fn term_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::cursor::MoveTo;
    use crossterm::style::{Attribute, Print, ResetColor, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};

    let parsed = frame_ops(heap, arg(args, 0), "term-draw", "vector (a frame)")?;
    let clear_t = value::intern("clear");
    let text_t = value::intern("text");
    let cursor_t = value::intern("cursor");
    let rect_t = value::intern("rect");
    let mut out: Vec<u8> = Vec::new();
    for (tag, parts) in parsed {
        if tag == clear_t {
            crossterm::queue!(out, Clear(ClearType::All)).map_err(term_err)?;
        } else if tag == rect_t {
            // [:rect row col w h face] — fill the block by printing `w` spaces in the
            // face on each of the `h` rows, so the face `:bg` (or `:reverse`) shows.
            let row = expect_int(heap, "term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "term-draw", arg(&parts, 2))?;
            let w = expect_int(heap, "term-draw", arg(&parts, 3))?.max(0) as usize;
            let h = expect_int(heap, "term-draw", arg(&parts, 4))?;
            let face = parts.get(5).copied().unwrap_or(Value::Nil);
            let fill = " ".repeat(w);
            for i in 0..h.max(0) {
                crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row + i)))
                    .map_err(term_err)?;
                apply_face(&mut out, heap, face)?;
                crossterm::queue!(
                    out,
                    Print(&fill),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )
                .map_err(term_err)?;
            }
        } else if tag == cursor_t {
            use crate::gui::CursorStyle;
            use crossterm::cursor::SetCursorStyle;
            let row = expect_int(heap, "term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "term-draw", arg(&parts, 2))?;
            crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row))).map_err(term_err)?;
            // honour the optional style keyword so the caret shape matches the GUI
            match cursor_style_from(parts.get(3).copied().unwrap_or(Value::Nil)) {
                CursorStyle::Bar => {
                    crossterm::queue!(out, SetCursorStyle::SteadyBar).map_err(term_err)?
                }
                CursorStyle::Underline => {
                    crossterm::queue!(out, SetCursorStyle::SteadyUnderScore).map_err(term_err)?
                }
                CursorStyle::Block => {
                    crossterm::queue!(out, SetCursorStyle::SteadyBlock).map_err(term_err)?
                }
            }
        } else if tag == text_t {
            let row = expect_int(heap, "term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "term-draw", arg(&parts, 2))?;
            let s = expect_string(heap, "term-draw", arg(&parts, 3))?;
            crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row))).map_err(term_err)?;
            apply_face(&mut out, heap, parts.get(4).copied().unwrap_or(Value::Nil))?;
            crossterm::queue!(out, Print(s), SetAttribute(Attribute::Reset), ResetColor)
                .map_err(term_err)?;
        }
    }
    write_term_bytes(&out).map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-raw-enter)` — raw mode only: no alternate screen, the cursor stays
/// visible, scrollback is preserved. The seam for an *inline* line editor (the
/// self-hosted REPL, std/lineedit.blsp), as opposed to `term-enter` which takes
/// over the whole screen for a full-screen TUI. Pair with `term-raw-leave`.
///
/// Defensively *shows the cursor and disables mouse capture* on entry: terminal
/// state persists across processes, so a prior full-screen app (`term-enter` hides
/// the cursor + captures the mouse) that exited without restoring — a crash, a
/// hard `Ctrl-C`, a killed observer — would otherwise leave the inline editor with
/// no cursor and mouse-movement escape sequences injected as input. The inline
/// editor only runs on a TTY (the REPL gates `lineedit-read` on stdin+stdout being
/// terminals; the piped path uses `read-line`), so these escapes never reach a
/// redirected stream. Idempotent: showing a visible cursor / disabling inactive
/// capture are no-ops.
fn term_raw_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    crossterm::terminal::enable_raw_mode().map_err(term_err)?;
    crossterm::execute!(std::io::stdout(), Show, DisableMouseCapture).map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-raw-leave)` — leave raw mode (the teardown for `term-raw-enter`).
/// Idempotent with the panic-path `restore_terminal`.
fn term_raw_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    crossterm::terminal::disable_raw_mode().map_err(term_err)?;
    Ok(Value::Nil)
}

/// `(term-emit ops)` — inline, relative-motion rendering for an in-place editor
/// that must not take over the screen (unlike `term-draw`, which paints absolute
/// cells on the alternate screen). Interprets a vector of op vectors, queued then
/// flushed once so a repaint doesn't tear:
///   `[:print str]` / `[:print str face]`  print at the cursor (face via apply_face)
///   `[:cr]`                                carriage return to column 0
///   `[:nl]`                                newline (`"\r\n"`)
///   `[:up n]` / `[:down n]`                move the cursor n rows
///   `[:col n]`                             move to absolute column n (0-based)
///   `[:clear-eol]`                         clear from the cursor to end of line
///   `[:clear-below]`                       clear from the cursor to end of screen
///   `[:clear-screen]`                      clear the whole screen, cursor to (0,0)
/// Unknown ops are skipped (forward-compatible, like `term-draw`).
fn term_emit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::cursor::{MoveDown, MoveToColumn, MoveUp};
    use crossterm::style::{Attribute, Print, ResetColor, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};

    let parsed = frame_ops(heap, arg(args, 0), "term-emit", "vector (ops)")?;
    let print_t = value::intern("print");
    let cr_t = value::intern("cr");
    let nl_t = value::intern("nl");
    let up_t = value::intern("up");
    let down_t = value::intern("down");
    let col_t = value::intern("col");
    let clear_eol_t = value::intern("clear-eol");
    let clear_below_t = value::intern("clear-below");
    let clear_screen_t = value::intern("clear-screen");
    let mut out: Vec<u8> = Vec::new();
    for (tag, parts) in parsed {
        if tag == print_t {
            let s = expect_string(heap, "term-emit", arg(&parts, 1))?;
            apply_face(&mut out, heap, parts.get(2).copied().unwrap_or(Value::Nil))?;
            crossterm::queue!(out, Print(s), SetAttribute(Attribute::Reset), ResetColor)
                .map_err(term_err)?;
        } else if tag == cr_t {
            crossterm::queue!(out, MoveToColumn(0)).map_err(term_err)?;
        } else if tag == nl_t {
            crossterm::queue!(out, Print("\r\n")).map_err(term_err)?;
        } else if tag == up_t {
            let n = expect_int(heap, "term-emit", arg(&parts, 1))?;
            if n > 0 {
                crossterm::queue!(out, MoveUp(clamp_u16(n))).map_err(term_err)?;
            }
        } else if tag == down_t {
            let n = expect_int(heap, "term-emit", arg(&parts, 1))?;
            if n > 0 {
                crossterm::queue!(out, MoveDown(clamp_u16(n))).map_err(term_err)?;
            }
        } else if tag == col_t {
            let n = expect_int(heap, "term-emit", arg(&parts, 1))?;
            crossterm::queue!(out, MoveToColumn(clamp_u16(n))).map_err(term_err)?;
        } else if tag == clear_eol_t {
            crossterm::queue!(out, Clear(ClearType::UntilNewLine)).map_err(term_err)?;
        } else if tag == clear_below_t {
            crossterm::queue!(out, Clear(ClearType::FromCursorDown)).map_err(term_err)?;
        } else if tag == clear_screen_t {
            crossterm::queue!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))
                .map_err(term_err)?;
        }
    }
    write_term_bytes(&out).map_err(term_err)?;
    Ok(Value::Nil)
}

/// The face-map keys (`:fg`/`:bg`/`:bold`/…) interned once for the whole process,
/// not re-interned per text op per frame on the render path — the same pre-intern
/// the frame dispatchers do for their op tags. Keyword interning is global and
/// append-only, so these stay valid for the process's life.
struct FaceKeys {
    fg: Value,
    bg: Value,
    bold: Value,
    italic: Value,
    underline: Value,
    reverse: Value,
    family: Value,
    scale: Value,
}
static FACE_KEYS: std::sync::LazyLock<FaceKeys> = std::sync::LazyLock::new(|| FaceKeys {
    fg: value::kw("fg"),
    bg: value::kw("bg"),
    bold: value::kw("bold"),
    italic: value::kw("italic"),
    underline: value::kw("underline"),
    reverse: value::kw("reverse"),
    family: value::kw("family"),
    scale: value::kw("scale"),
});

/// Apply a face map (`{:fg :red :bg :blue :bold true :reverse true}`) as
/// crossterm style commands. A non-map (or nil) face is a no-op. Unknown colour
/// names are skipped. Callers reset attributes after the text.
fn apply_face<W: std::io::Write>(out: &mut W, heap: &Heap, face: Value) -> Result<(), LispError> {
    use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
    let Value::Map(id) = face else { return Ok(()) };
    let k = &*FACE_KEYS;
    if let Some(fg) = heap.map_get(id, k.fg).and_then(|v| color_of(heap, v)) {
        crossterm::queue!(out, SetForegroundColor(fg)).map_err(term_err)?;
    }
    if let Some(bg) = heap.map_get(id, k.bg).and_then(|v| color_of(heap, v)) {
        crossterm::queue!(out, SetBackgroundColor(bg)).map_err(term_err)?;
    }
    if heap.map_get(id, k.bold).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Bold)).map_err(term_err)?;
    }
    if heap.map_get(id, k.italic).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Italic)).map_err(term_err)?;
    }
    if heap.map_get(id, k.underline).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Underlined)).map_err(term_err)?;
    }
    if heap.map_get(id, k.reverse).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Reverse)).map_err(term_err)?;
    }
    Ok(())
}

/// Brood truthiness for a face flag: only `nil`/`false` are falsy.
fn face_truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// A face colour value to a crossterm `Color`. A palette keyword (`:red`,
/// `:dark-grey`, …) maps to the terminal's *named* colour, so it honours the
/// user's terminal theme; an explicit `[r g b]` vector or `"#rrggbb"` string maps
/// to a true-colour cell (`Color::Rgb`) — the same RGB the GUI frontend paints, so
/// a curated palette renders identically in both.
fn color_of(heap: &Heap, v: Value) -> Option<crossterm::style::Color> {
    use crossterm::style::Color;
    if let Value::Keyword(s) = v {
        return Some(match value::symbol_name(s).as_str() {
            "black" => Color::Black,
            "red" => Color::Red,
            "green" => Color::Green,
            "yellow" => Color::Yellow,
            "blue" => Color::Blue,
            "magenta" => Color::Magenta,
            "cyan" => Color::Cyan,
            "white" => Color::White,
            "grey" | "gray" => Color::Grey,
            "dark-grey" | "dark-gray" => Color::DarkGrey,
            _ => return None,
        });
    }
    face_rgb(heap, v).map(|[r, g, b]| Color::Rgb { r, g, b })
}

/// Clamp a Brood int to a terminal coordinate (crossterm uses `u16`).
fn clamp_u16(n: i64) -> u16 {
    n.clamp(0, u16::MAX as i64) as u16
}

// ---- the GUI frontend (ADR-046, feature "gui") ------------------------------
//
// `gui-*` mirror `term-*`: a second frontend that paints the *same* render-op
// protocol (a frame is the same Brood data) to a native window and reads keys
// back in the same encoding. The window/loop machinery lives in `crate::gui`
// (behind the `gui` feature); these primitives just translate Brood `Value`s ⇄
// the plain `gui::Op`/`gui::Key`/`gui::Face` the backend speaks. A composite
// "broadcast" display in std/observer.blsp drives term + gui (+ remote later)
// from one frame — so the frontends can't drift. Without `--features gui` the
// backend functions return a clear "rebuild with --features gui" error.

/// A face colour keyword (`:red`, `:dark-grey`, …) to an RGB triple for the GUI
/// framebuffer. The same palette `color_of` maps to crossterm `Color`s, so the
/// two frontends agree on what `:red` looks like.
fn color_rgb(v: Value) -> Option<[u8; 3]> {
    let Value::Keyword(s) = v else { return None };
    Some(match value::symbol_name(s).as_str() {
        "black" => [0x00, 0x00, 0x00],
        "red" => [0xcd, 0x31, 0x31],
        "green" => [0x0d, 0xbc, 0x79],
        "yellow" => [0xe5, 0xe5, 0x10],
        "blue" => [0x24, 0x72, 0xc8],
        "magenta" => [0xbc, 0x3f, 0xbc],
        "cyan" => [0x11, 0xa8, 0xcd],
        "white" => [0xe5, 0xe5, 0xe5],
        "grey" | "gray" => [0x80, 0x80, 0x80],
        "dark-grey" | "dark-gray" => [0x50, 0x50, 0x50],
        _ => return None,
    })
}

/// Resolve a face colour VALUE to an RGB triple — the one place every frontend
/// agrees on what a colour means. Accepts a palette keyword (`:red`, via
/// `color_rgb`), an explicit `[r g b]` vector (each channel clamped to 0..255), or
/// a `"#rgb"` / `"#rrggbb"` hex string. Anything else is `None` (the default face).
/// This is what lets a UI curate a soft RGB palette instead of the harsh
/// ANSI-16 keywords — and the `:vspans` fast path shares it too.
fn face_rgb(heap: &Heap, v: Value) -> Option<[u8; 3]> {
    match v {
        Value::Keyword(_) => color_rgb(v),
        Value::Vector(id) => {
            let xs = heap.vector(id);
            if xs.len() == 3 {
                let chan = |k: usize| match xs[k] {
                    Value::Int(n) => n.clamp(0, 255) as u8,
                    _ => 0,
                };
                Some([chan(0), chan(1), chan(2)])
            } else {
                None
            }
        }
        Value::Str(id) => parse_hex_color(heap.string(id)),
        _ => None,
    }
}

/// Parse a `"#rgb"` or `"#rrggbb"` hex colour to an RGB triple. `None` for any
/// other shape (no leading `#`, a bad length, or a non-hex digit). The 3-digit
/// shorthand expands each nibble (`#f0a` → `[0xff 0x00 0xaa]`).
fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    let b = h.as_bytes();
    match h.len() {
        3 => {
            let d = |i: usize| (b[i] as char).to_digit(16).map(|n| (n * 17) as u8);
            Some([d(0)?, d(1)?, d(2)?])
        }
        6 => {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            Some([p(0)?, p(2)?, p(4)?])
        }
        _ => None,
    }
}

/// Resolve a face map (`{:fg :red :bg :blue :bold true :reverse true}`) into the
/// plain `gui::Face` the backend renders. A non-map face is the default face.
fn gui_face(heap: &Heap, face: Value) -> crate::gui::Face {
    let mut f = crate::gui::Face::default();
    let Value::Map(id) = face else { return f };
    let k = &*FACE_KEYS;
    f.fg = heap.map_get(id, k.fg).and_then(|v| face_rgb(heap, v));
    f.bg = heap.map_get(id, k.bg).and_then(|v| face_rgb(heap, v));
    f.bold = heap.map_get(id, k.bold).is_some_and(face_truthy);
    f.italic = heap.map_get(id, k.italic).is_some_and(face_truthy);
    f.underline = heap.map_get(id, k.underline).is_some_and(face_truthy);
    f.reverse = heap.map_get(id, k.reverse).is_some_and(face_truthy);
    // `:family` is a keyword naming a registered font family; carry its interned
    // id so the renderer can pick the matching font set (`:mono` / unknown → default).
    f.family = match heap.map_get(id, k.family) {
        Some(Value::Keyword(s)) => Some(s),
        _ => None,
    };
    // `:scale n` (GUI only, ADR-079): draw the op's text n× larger, in an n×n cell
    // block. Clamp to 1..=GUI_MAX_SCALE — a non-positive value falls back to the
    // default 1, and the cap bounds the per-op framebuffer work + glyph cache.
    if let Some(Value::Int(n)) = heap.map_get(id, k.scale) {
        f.scale = n.clamp(1, GUI_MAX_SCALE as i64) as u16;
    }
    f
}

/// Upper bound on a face's `:scale` (a `:scale 3` glyph already covers 9 cells; the
/// cap keeps a stray huge value from blowing up framebuffer work + the glyph cache).
const GUI_MAX_SCALE: u16 = 16;

/// Read a window-id argument (the integer `gui-open` returned) for the windowed
/// primitives. Negative ids clamp to 0 (no such window → a clean "not open" error).
fn gui_window_id(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    Ok(expect_int(heap, who, v)?.max(0) as u64)
}

/// `(gui-open)` / `(gui-open title)` — open a new native window and return its integer
/// id, optionally with a title-bar string (else a default `brood observer #id`). Its
/// key/mouse input is delivered to the **calling process's mailbox** (ADR-058), so the
/// observer parks in `(receive)` rather than pinning a worker in a blocking poll.
/// Starts the GUI thread on the first call; each call is an independent window.
fn gui_open(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let title = match arg(args, 0) {
        Value::Nil => None,
        v => Some(expect_string(heap, "gui-open", v)?),
    };
    let size = match arg(args, 1) {
        Value::Nil => None,
        w => Some((
            expect_int(heap, "gui-open", w)? as f64,
            expect_int(heap, "gui-open", arg(args, 2))? as f64,
        )),
    };
    let id =
        crate::gui::open(crate::process::self_pid(), title, size).map_err(LispError::runtime)?;
    Ok(Value::Int(id as i64))
}

/// `(gui-close id)` — close window `id` (the teardown for `gui-open`; idempotent).
fn gui_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-close", arg(args, 0))?;
    crate::gui::close(id).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-title! id text)` — set window `id`'s OS title-bar text at runtime.
fn gui_title(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-title!", arg(args, 0))?;
    let title = expect_string(heap, "gui-title!", arg(args, 1))?;
    crate::gui::title(id, title).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-icon! id rgba w h)` — set window `id`'s taskbar/title-bar icon from raw RGBA
/// pixels (a vector of `w*h*4` byte ints, row-major).
fn gui_icon(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-icon!", arg(args, 0))?;
    let w = expect_int(heap, "gui-icon!", arg(args, 2))? as u32;
    let h = expect_int(heap, "gui-icon!", arg(args, 3))? as u32;
    let rgba: Vec<u8> = match arg(args, 1) {
        Value::Vector(vid) => heap
            .vector(vid)
            .iter()
            .map(|v| match v {
                Value::Int(i) => *i as u8,
                _ => 0,
            })
            .collect(),
        _ => {
            return Err(LispError::runtime(
                "gui-icon!: rgba must be a vector of bytes".to_string(),
            ))
        }
    };
    crate::gui::icon(id, rgba, w, h).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-focus id)` — raise window `id` and give it OS keyboard focus (un-minimising
/// it). Lets an app surface an already-open singleton window instead of opening a
/// duplicate. Errors only if `id` isn't a live window.
fn gui_focus(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-focus", arg(args, 0))?;
    crate::gui::focus(id).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-grab-cursor id on)` — confine the pointer to window `id` while `on` is
/// truthy, release it otherwise.
fn gui_grab_cursor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-grab-cursor", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::gui::grab(id, on).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-fullscreen! id on)` — make window `id` borderless-fullscreen (`on` truthy)
/// or restore it to a normal window.
fn gui_fullscreen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-fullscreen!", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::gui::fullscreen(id, on).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-maximize! id on)` — maximise window `id` (`on` truthy) or restore it,
/// keeping the title bar / decorations (unlike fullscreen).
fn gui_maximize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-maximize!", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::gui::maximize(id, on).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-size id)` — window `id`'s size as `[cols rows]` (character cells), same
/// shape as `term-size`.
fn gui_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-size", arg(args, 0))?;
    let (cols, rows) = crate::gui::size(id).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector(vec![Value::Int(cols as i64), Value::Int(rows as i64)]))
}

/// A held `gui::Key` as the same Brood value `gui-open` delivers for that press —
/// a 1-char string, else a `:ctrl-…` / `:alt-…` / `:ctrl-meta-…` / named keyword —
/// so an app can compare `(gui-held-key id)` directly against the key it last saw.
fn gui_key_to_value(heap: &mut Heap, k: crate::gui::Key) -> Value {
    use crate::gui::Key;
    match k {
        Key::Char(c) => heap.alloc_string(&c.to_string()),
        Key::Ctrl(c) => Value::Keyword(value::intern(&format!("ctrl-{c}"))),
        Key::Alt(c) => Value::Keyword(value::intern(&format!("alt-{c}"))),
        Key::CtrlAlt(c) => Value::Keyword(value::intern(&format!("ctrl-meta-{c}"))),
        Key::Named(s) => Value::Keyword(value::intern(s)),
    }
}

/// `(gui-held-key id)` — the key window `id` currently sees as physically held (the
/// same value its press delivered), or nil when none. Tracked from press/release
/// transitions, not winit's unreliable `ke.repeat`, so it's the source of truth a
/// consumer-paced key repeat polls to stop the instant the key is up — making a
/// missed key-up unable to cause runaway repeat (ADR-086).
fn gui_held_key(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "gui-held-key", arg(args, 0))?;
    match crate::gui::held_key(id).map_err(LispError::runtime)? {
        Some(k) => Ok(gui_key_to_value(heap, k)),
        None => Ok(Value::Nil),
    }
}

/// `(gui-draw id frame)` — paint a frame (the same op vector `term-draw` takes) to
/// window `id`. Parses the ops into plain `gui::Op`s (it has heap access) and ships
/// them to the GUI thread. Unknown ops are skipped (forward-compatible).
fn gui_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let win = gui_window_id(heap, "gui-draw", arg(args, 0))?;
    let parsed = frame_ops(heap, arg(args, 1), "gui-draw", "vector (a frame)")?;
    let clear_t = value::intern("clear");
    let text_t = value::intern("text");
    let cursor_t = value::intern("cursor");
    let cursor_zone_t = value::intern("cursor-zone");
    let col_resize_t = value::intern("col-resize");
    let row_resize_t = value::intern("row-resize");
    let vspans_t = value::intern("vspans");
    let cells_t = value::intern("cells");
    let rect_t = value::intern("rect");
    let mut ops = Vec::with_capacity(parsed.len());
    for (tag, parts) in parsed {
        if tag == clear_t {
            ops.push(crate::gui::Op::Clear);
        } else if tag == cursor_t {
            let row = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let style = cursor_style_from(parts.get(3).copied().unwrap_or(Value::Nil));
            ops.push(crate::gui::Op::Cursor { row, col, style });
        } else if tag == rect_t {
            // [:rect row col w h face] — fill a w×h cell block with the face background.
            let row = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let w = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 3))?);
            let h = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 4))?);
            let face = gui_face(heap, parts.get(5).copied().unwrap_or(Value::Nil));
            ops.push(crate::gui::Op::Rect {
                row,
                col,
                w,
                h,
                face,
            });
        } else if tag == text_t {
            let row = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let s = expect_string(heap, "gui-draw", arg(&parts, 3))?;
            let face = gui_face(heap, parts.get(4).copied().unwrap_or(Value::Nil));
            ops.push(crate::gui::Op::Text { row, col, s, face });
        } else if tag == cursor_zone_t {
            // [:cursor-zone x y w h shape] — a hover hot-zone. Unknown shape: skip.
            let x = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let y = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let w = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 3))?);
            let h = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 4))?);
            let shape = match parts.get(5) {
                Some(Value::Keyword(s)) if *s == col_resize_t => {
                    Some(crate::gui::CursorShape::ColResize)
                }
                Some(Value::Keyword(s)) if *s == row_resize_t => {
                    Some(crate::gui::CursorShape::RowResize)
                }
                _ => None,
            };
            if let Some(shape) = shape {
                ops.push(crate::gui::Op::CursorZone { x, y, w, h, shape });
            }
        } else if tag == vspans_t {
            // [:vspans row0 col0 cols] — a batch of vertical column-spans. `cols`
            // is a vector (one per cell-column) of `[height color]` segments; the
            // per-cell fill happens in `gui::paint`, so the Brood side builds only
            // O(columns) data instead of an op-per-cell frame. `color` is a face
            // colour keyword (`:red`), an `[r g b]` triple (0..255), or nil (the
            // background shows through).
            let row0 = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col0 = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let col_vals: Vec<Value> = match arg(&parts, 3) {
                Value::Vector(id) => heap.vector(id).to_vec(),
                _ => Vec::new(),
            };
            let mut cols = Vec::with_capacity(col_vals.len());
            for cv in col_vals {
                let seg_vals: Vec<Value> = match cv {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => Vec::new(),
                };
                let mut segs = Vec::with_capacity(seg_vals.len());
                for sv in seg_vals {
                    let s: Vec<Value> = match sv {
                        Value::Vector(id) => heap.vector(id).to_vec(),
                        _ => continue,
                    };
                    if s.len() >= 2 {
                        let h = clamp_u16(expect_int(heap, "gui-draw", s[0])?);
                        segs.push((h, span_color(heap, s[1])));
                    }
                }
                cols.push(segs);
            }
            ops.push(crate::gui::Op::VSpans { row0, col0, cols });
        } else if tag == cells_t {
            // [:cells row0 col0 w aspect bits color] — blit a whole BITBOARD in one op.
            // `bits` is an arbitrary-precision integer (set bit `y*w + x` = cell `(x,y)`
            // live); each live cell fills an `aspect`×1 screen-cell block in `color`,
            // anchored at screen cell `(row0, col0)`. The set-bit enumeration + rect
            // expansion run natively in `gui::paint` (O(live)), so a frame of thousands
            // of cells is ONE op for the Brood side. `color` is a face keyword / [r g b]
            // / nil (as `:vspans`). GUI-only; the terminal has no arm, so it's skipped.
            let row0 = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 1))?);
            let col0 = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 2))?);
            let w = expect_int(heap, "gui-draw", arg(&parts, 3))?.max(1) as u32;
            let aspect = clamp_u16(expect_int(heap, "gui-draw", arg(&parts, 4))?).max(1);
            // The board may be a bignum OR a `bitset` (a refc-shared `Str` of raw bytes) —
            // decode either to little-endian set-bit bytes for the representation-agnostic paint.
            let bytes = match arg(&parts, 5) {
                Value::Str(id) => match heap.local_shared_blob(id) {
                    Some(blob) => blob.as_bytes().to_vec(),
                    None => heap.string(id).as_bytes().to_vec(),
                },
                v => expect_bigint(heap, "gui-draw", v)?.magnitude().to_bytes_le(),
            };
            let color = span_color(heap, arg(&parts, 6));
            ops.push(crate::gui::Op::Cells { row0, col0, w, aspect, bytes, color });
        }
    }
    crate::gui::draw(win, ops).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// A `:vspans` segment colour: a face colour keyword (`:red` → the GUI palette),
/// an explicit `[r g b]` triple, or a `"#rrggbb"` hex string (all via the shared
/// `face_rgb`); anything else is `None` — "transparent", leaving the background
/// showing.
fn span_color(heap: &Heap, v: Value) -> Option<[u8; 3]> {
    face_rgb(heap, v)
}

/// The cursor style from a `[:cursor row col style]` op's optional 4th element: a
/// `:bar` / `:underline` keyword, else (`:block`, nil, or anything unknown) the
/// default `Block`. Shared by both frontends so the caret shape agrees.
fn cursor_style_from(v: Value) -> crate::gui::CursorStyle {
    use crate::gui::CursorStyle;
    match v {
        Value::Keyword(s) => match value::symbol_name(s).as_str() {
            "bar" => CursorStyle::Bar,
            "underline" => CursorStyle::Underline,
            _ => CursorStyle::Block,
        },
        _ => CursorStyle::Block,
    }
}

/// Read a `:height` value from a font spec as a pixel size (int or float), or None.
fn font_px(heap: &Heap, id: crate::core::value::MapId) -> Option<f32> {
    match heap.map_get(id, value::kw("height")) {
        Some(Value::Int(n)) => Some(n as f32),
        Some(Value::Float(f)) => Some(f as f32),
        _ => None,
    }
}

/// `(gui-font! spec)` / `(gui-font! id spec)` — set a cell font from `spec`, a map
/// `{:family <keyword> :height <px>}` (either key optional): `:family` picks a
/// registered font family (the bundled `:mono`, or one added by
/// `gui-font-register`), `:height` the cell pixel size. With one argument it sets
/// the **global default** (every open window + any opened later); with a leading
/// window `id` it retunes **just that window**, leaving the global default and
/// other windows alone — so two windows can run different fonts. (Per-section
/// fonts within a window come from a face's `:family`/`:scale`.) Returns nil.
fn gui_font(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // (gui-font! spec) → global default; (gui-font! id spec) → just window `id`.
    let (win, spec) = if args.len() >= 2 {
        (
            Some(gui_window_id(heap, "gui-font!", arg(args, 0))?),
            arg(args, 1),
        )
    } else {
        (None, arg(args, 0))
    };
    let Value::Map(m) = spec else {
        return Err(LispError::wrong_type(
            heap,
            "gui-font!",
            "map (a font spec)",
            spec,
        ));
    };
    let family = match heap.map_get(m, value::kw("family")) {
        Some(Value::Keyword(s)) => Some(s),
        _ => None,
    };
    crate::gui::font(win, family, font_px(heap, m)).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-inset! px)` — set the window content inset (logical pixels): a blank margin
/// before the cell grid on every edge, so text doesn't sit flush against the window
/// frame. Applies to every open window + the default for ones opened later. The grid
/// loses `2*px` per axis (fewer cells) and re-renders. GUI only.
fn gui_inset(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let px = match arg(args, 0) {
        Value::Int(n) => n.max(0) as f32,
        Value::Float(f) => f.max(0.0) as f32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "gui-inset!",
                "a number (pixels)",
                other,
            ))
        }
    };
    crate::gui::inset(px).map_err(LispError::runtime)?;
    Ok(Value::Nil)
}

/// `(gui-font-register name styles)` — register font family `name` (a keyword) from
/// `styles`, a map of style → TTF file path: `{:regular "…" :bold "…" :italic "…"
/// :bold-italic "…"}`. Only `:regular` is required; a missing style reuses the
/// regular file (so a single-file family works). The fonts are read here and parsed
/// on the GUI thread; afterwards a face's `:family <name>` selects them. Returns
/// `name`.
fn gui_font_register(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "gui-font-register",
                "keyword",
                other,
            ))
        }
    };
    let Value::Map(id) = arg(args, 1) else {
        return Err(LispError::wrong_type(
            heap,
            "gui-font-register",
            "map (style → path)",
            arg(args, 1),
        ));
    };
    // a style's path, or None when the key is absent/nil
    let path = |key: &str| -> Result<Option<String>, LispError> {
        match heap.map_get(id, value::kw(key)) {
            None | Some(Value::Nil) => Ok(None),
            Some(v) => Ok(Some(expect_string(heap, "gui-font-register", v)?)),
        }
    };
    let read = |p: &str| -> Result<Vec<u8>, LispError> {
        std::fs::read(p).map_err(|e| LispError::runtime(format!("gui-font-register: {p}: {e}")))
    };
    let regular_path = path("regular")?
        .ok_or_else(|| LispError::runtime("gui-font-register: a :regular path is required"))?;
    let regular = read(&regular_path)?;
    // each missing style falls back to the regular file's bytes
    let style = |key: &str| -> Result<Vec<u8>, LispError> {
        match path(key)? {
            Some(p) => read(&p),
            None => Ok(regular.clone()),
        }
    };
    let bold = style("bold")?;
    let italic = style("italic")?;
    let bold_italic = style("bold-italic")?;
    crate::gui::register_family(name, regular, bold, italic, bold_italic)
        .map_err(LispError::runtime)?;
    Ok(Value::Keyword(name))
}

/// `(mailbox-size pid)` — the number of queued messages in a local process's
/// mailbox, or `nil` for a remote/dead pid. The one process-introspection
/// accessor Brood can't reach (the queue lives behind the scheduler registry);
/// `std/observer.blsp` assembles everything else (id, liveness) from Brood.
fn mailbox_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(crate::process::mailbox_len(id)
                .map(|n| Value::Int(n as i64))
                .unwrap_or(Value::Nil))
        }
        Value::Pid { .. } => Ok(Value::Nil),
        other => Err(LispError::wrong_type(heap, "mailbox-size", "pid", other)),
    }
}

/// `(process-info pid)` — a snapshot map of a **live local** process, or `nil`
/// for a remote/dead pid (a non-pid is a type error). The fields are all
/// kernel-internal, so the map is assembled here from the registry / scheduler /
/// name / monitor tables (ADR-051):
///
///   `{:id <int> :node <kw> :name <kw|nil> :status <kw> :mailbox <int>
///     :monitored-by <int> :parent <int|nil>}`
///
/// `:status` is `:running` / `:waiting` (parked in `receive`). `:name` is the
/// registered name or nil. `:parent` is the spawner's id (nil for the root).
/// `:memory` (per-process bytes) joins once the kernel tracks it, and `:status`
/// sharpens when an explicit state enum lands (the observer tolerates the gap).
/// Each accessor takes one lock independently, so no two are held at once.
fn process_info(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Dead/unknown pid → nil (matches `mailbox-size`).
            if !crate::process::is_alive(id) {
                return Ok(Value::Nil);
            }
            let name = crate::dist::name_for_pid(id)
                .map(Value::Keyword)
                .unwrap_or(Value::Nil);
            let status = crate::process::process_status(id)
                .map(value::kw)
                .unwrap_or(Value::Nil);
            let mailbox = Value::Int(crate::process::mailbox_len(id).unwrap_or(0) as i64);
            let monitored = Value::Int(crate::process::monitored_by(id) as i64);
            // `:parent` is the spawner's id, or nil for the root.
            let parent = crate::process::parent_of(id)
                .map(|p| Value::Int(p as i64))
                .unwrap_or(Value::Nil);
            // `:memory` — the process's LOCAL heap footprint (bytes), published on
            // its last `receive`; 0 for a process that has never received.
            let memory = Value::Int(crate::process::process_mem(id).unwrap_or(0) as i64);
            // `:collections` — the process's cumulative GC count, republished on
            // its last `receive` (0 for one that has never received). The signal
            // for "is this process churning memory?" in the observer.
            let collections = Value::Int(crate::process::process_gc_runs(id).unwrap_or(0) as i64);
            // `:reductions` — the process's cumulative reduction count (Erlang's
            // scheduling unit), updated every scheduling quantum. The observer's
            // "is this process doing work / busy?" signal. Exact for spawned
            // processes; coarse (whole-budget increments) for the root.
            let reductions = Value::Int(crate::process::process_reductions(id).unwrap_or(0) as i64);
            let pairs = vec![
                (value::kw("id"), Value::Int(id as i64)),
                // The process's actual pid value (not just its numeric id), so a
                // caller — e.g. the observer's kill command — can act on the
                // process directly with `exit`/`send`/`monitor`.
                (value::kw("pid"), Value::Pid { node, id }),
                (value::kw("node"), Value::Keyword(node)),
                (value::kw("name"), name),
                (value::kw("status"), status),
                (value::kw("mailbox"), mailbox),
                (value::kw("monitored-by"), monitored),
                (value::kw("parent"), parent),
                (value::kw("memory"), memory),
                (value::kw("collections"), collections),
                (value::kw("reductions"), reductions),
            ];
            Ok(heap.map_from_pairs(pairs))
        }
        Value::Pid { .. } => Ok(Value::Nil),
        other => Err(LispError::wrong_type(heap, "process-info", "pid", other)),
    }
}

/// `(string->number s)` — parse `s` as an integer if it is one, else as a float,
/// else `nil`. The inverse of `number->string`. A robust parse-or-nil can't be
/// expressed over `read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.
fn string_to_number(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->number", arg(args, 0))?;
    if let Ok(i) = s.parse::<i64>() {
        Ok(Value::Int(i))
    } else if let Ok(n) = s.parse::<num_bigint::BigInt>() {
        // An integer too big for i64 is a bignum — mirroring the reader's
        // over-range literal path — NOT a lossy f64 (which silently rounded
        // `(number->string big)` away from round-tripping, kernel audit).
        // Reaching here means the i64 parse failed, so `n` is out of range
        // and `alloc_bigint`'s no-demotion invariant holds.
        Ok(heap.alloc_bigint(n))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::Float(f))
    } else {
        Ok(Value::Nil)
    }
}

// ---------- filesystem ----------
// Mechanism only: existence / directory reflection so the Brood module system and
// the project test runner can resolve load paths and discover test files. Path
// manipulation and all policy live in Brood (`std/prelude.blsp`, `std/project.blsp`).

/// `(cwd)` — the process's current working directory as a string.
fn cwd(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_dir() {
        Ok(p) => Ok(heap.alloc_string(&p.to_string_lossy())),
        Err(e) => {
            Err(LispError::runtime(format!("cwd: {}", e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    }
}

/// `(file-exists? path)` — true if a file or directory exists at `path`.
fn file_exists(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-exists?", arg(args, 0))?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// `(dir? path)` — true if `path` exists and is a directory.
fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "dir?", arg(args, 0))?;
    Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
}

/// `(list-dir path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "list-dir", arg(args, 0))?;
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) => {
            return Err(LispError::runtime(format!("list-dir: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    };
    names.sort();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
}

/// `(make-dir path)` — create `path` and any missing parents (like `mkdir -p`).
/// Returns nil. Used by the project scaffolder (`nest new`).
fn make_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "make-dir", arg(args, 0))?;
    std::fs::create_dir_all(&path).map_err(|e| {
        LispError::runtime(format!("make-dir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

/// `(spit path content)` — write `content` (a string) to `path`, replacing any
/// existing file. Returns nil. The write-side counterpart to `load`.
fn spit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let path = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string path", pv)),
    };
    let cv = arg(args, 1);
    let content = match cv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string content", cv)),
    };
    std::fs::write(&path, content).map_err(|e| {
        LispError::runtime(format!("spit: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

/// `(%sha256 s)` — the lowercase hex SHA-256 of `s`'s UTF-8 bytes. The single
/// hashing mechanism for the package manager (ADR-037); per-file hashing
/// (`(%sha256 (slurp p))`) and the canonical directory-tree hash are Brood over
/// it (`std/package.blsp`), keeping a directory walk out of the kernel.
fn sha256_hex(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha256};
    let s = expect_string(heap, "%sha256", arg(args, 0))?;
    Ok(heap.alloc_string(&digest_to_hex(Sha256::digest(s.as_bytes()))))
}

/// Extract a vector or list of byte ints (0–255) from a `Value`.
fn collect_bytes(name: &'static str, bv: Value, heap: &mut Heap) -> Result<Vec<u8>, LispError> {
    match bv {
        Value::Vector(id) => {
            let vec = heap.vector(id).to_vec();
            vec.iter()
                .map(|v| match v {
                    Value::Int(n) if *n >= 0 && *n <= 255 => Ok(*n as u8),
                    other => Err(LispError::wrong_type(
                        heap,
                        name,
                        "byte int (0-255)",
                        *other,
                    )),
                })
                .collect::<Result<Vec<u8>, LispError>>()
        }
        Value::Pair(_) | Value::Nil => {
            let mut out = Vec::new();
            let mut cur = bv;
            loop {
                match cur {
                    Value::Nil => break,
                    Value::Pair(id) => {
                        let (h, t) = heap.pair(id);
                        match h {
                            Value::Int(n) if n >= 0 && n <= 255 => out.push(n as u8),
                            other => {
                                return Err(LispError::wrong_type(
                                    heap,
                                    name,
                                    "byte int (0-255)",
                                    other,
                                ))
                            }
                        }
                        cur = t;
                    }
                    other => return Err(LispError::wrong_type(heap, name, "proper list", other)),
                }
            }
            Ok(out)
        }
        other => Err(LispError::wrong_type(heap, name, "vector or list", other)),
    }
}

fn digest_to_hex(digest: impl AsRef<[u8]>) -> String {
    let bytes = digest.as_ref();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

/// `(%sha256-bytes bytes)` — hex SHA-256 of a vector or list of byte integers.
fn sha256_hex_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha256};
    let bytes = collect_bytes("%sha256-bytes", arg(args, 0), heap)?;
    Ok(heap.alloc_string(&digest_to_hex(Sha256::digest(&bytes))))
}

/// `(%sha1 s)` — lowercase hex SHA-1 of string `s`'s UTF-8 bytes.
fn sha1_hex(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha1::{Digest, Sha1};
    let s = expect_string(heap, "%sha1", arg(args, 0))?;
    Ok(heap.alloc_string(&digest_to_hex(Sha1::digest(s.as_bytes()))))
}

/// `(%sha1-bytes bytes)` — hex SHA-1 of a vector or list of byte integers.
fn sha1_hex_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha1::{Digest, Sha1};
    let bytes = collect_bytes("%sha1-bytes", arg(args, 0), heap)?;
    Ok(heap.alloc_string(&digest_to_hex(Sha1::digest(&bytes))))
}

/// `(%sha384 s)` — lowercase hex SHA-384 of string `s`'s UTF-8 bytes.
fn sha384_hex(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha384};
    let s = expect_string(heap, "%sha384", arg(args, 0))?;
    Ok(heap.alloc_string(&digest_to_hex(Sha384::digest(s.as_bytes()))))
}

/// `(%sha384-bytes bytes)` — hex SHA-384 of a vector or list of byte integers.
fn sha384_hex_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha384};
    let bytes = collect_bytes("%sha384-bytes", arg(args, 0), heap)?;
    Ok(heap.alloc_string(&digest_to_hex(Sha384::digest(&bytes))))
}

/// `(%sha512 s)` — lowercase hex SHA-512 of string `s`'s UTF-8 bytes.
fn sha512_hex(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha512};
    let s = expect_string(heap, "%sha512", arg(args, 0))?;
    Ok(heap.alloc_string(&digest_to_hex(Sha512::digest(s.as_bytes()))))
}

/// `(%sha512-bytes bytes)` — hex SHA-512 of a vector or list of byte integers.
fn sha512_hex_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use sha2::{Digest, Sha512};
    let bytes = collect_bytes("%sha512-bytes", arg(args, 0), heap)?;
    Ok(heap.alloc_string(&digest_to_hex(Sha512::digest(&bytes))))
}

/// `(%md5 s)` — lowercase hex MD5 of string `s`'s UTF-8 bytes.
fn md5_hex(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use md5::{Digest, Md5};
    let s = expect_string(heap, "%md5", arg(args, 0))?;
    Ok(heap.alloc_string(&digest_to_hex(Md5::digest(s.as_bytes()))))
}

/// `(%md5-bytes bytes)` — hex MD5 of a vector or list of byte integers.
fn md5_hex_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use md5::{Digest, Md5};
    let bytes = collect_bytes("%md5-bytes", arg(args, 0), heap)?;
    Ok(heap.alloc_string(&digest_to_hex(Md5::digest(&bytes))))
}

// ---- HMAC primitives -------------------------------------------------------

/// `(%hmac-sha256 key message)` — HMAC-SHA256 → lowercase hex.
fn hmac_sha256_fn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    let key = expect_string(heap, "%hmac-sha256", arg(args, 0))?;
    let msg = expect_string(heap, "%hmac-sha256", arg(args, 1))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .map_err(|e| LispError::runtime(format!("%hmac-sha256: {e}")))?;
    mac.update(msg.as_bytes());
    Ok(heap.alloc_string(&digest_to_hex(mac.finalize().into_bytes())))
}

/// `(%hmac-sha1 key message)` — HMAC-SHA1 → lowercase hex.
fn hmac_sha1_fn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    use sha1::Sha1;
    let key = expect_string(heap, "%hmac-sha1", arg(args, 0))?;
    let msg = expect_string(heap, "%hmac-sha1", arg(args, 1))?;
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes())
        .map_err(|e| LispError::runtime(format!("%hmac-sha1: {e}")))?;
    mac.update(msg.as_bytes());
    Ok(heap.alloc_string(&digest_to_hex(mac.finalize().into_bytes())))
}

/// `(%hmac-sha512 key message)` — HMAC-SHA512 → lowercase hex.
fn hmac_sha512_fn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha512;
    let key = expect_string(heap, "%hmac-sha512", arg(args, 0))?;
    let msg = expect_string(heap, "%hmac-sha512", arg(args, 1))?;
    let mut mac = Hmac::<Sha512>::new_from_slice(key.as_bytes())
        .map_err(|e| LispError::runtime(format!("%hmac-sha512: {e}")))?;
    mac.update(msg.as_bytes());
    Ok(heap.alloc_string(&digest_to_hex(mac.finalize().into_bytes())))
}

/// Run `git` with `args` (optionally in `cwd`), capturing stdout+stderr. The
/// shared mechanism behind the package manager's git primitives (ADR-037).
fn run_git(args: &[&str], cwd: Option<&str>) -> Result<std::process::Output, LispError> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.output().map_err(|e| {
        LispError::runtime(format!("git {}: {}", args.join(" "), e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("is `git` installed and on PATH?")
    })
}

/// Run a `git` subcommand that's expected to succeed; turn a non-zero exit into a
/// `LispError` carrying git's stderr.
fn git_or_err(args: &[&str], cwd: Option<&str>) -> Result<(), LispError> {
    let out = run_git(args, cwd)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(LispError::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED))
    }
}

/// `(%random-bytes n)` — `n` cryptographically-strong random bytes as a vector of
/// ints 0–255. Useful for generating keys, nonces, and salts.
fn random_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "%random-bytes", arg(args, 0))?;
    if !(0..=65536).contains(&n) {
        return Err(LispError::runtime(
            "%random-bytes: byte count must be in 0..=65536",
        ));
    }
    let mut bytes = vec![0u8; n as usize];
    getrandom::fill(&mut bytes)
        .map_err(|e| LispError::runtime(format!("%random-bytes: OS RNG unavailable: {e}")))?;
    let vals: Vec<Value> = bytes.iter().map(|&b| Value::Int(b as i64)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(%chacha20-encrypt key-bytes nonce-bytes plaintext-bytes)` — authenticated
/// encryption (ChaCha20-Poly1305). `key-bytes` must be exactly 32 bytes;
/// `nonce-bytes` must be exactly 12 bytes. Returns the ciphertext (plaintext
/// length + 16-byte Poly1305 authentication tag) as a byte vector.
fn chacha20_encrypt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
    let key_bytes = collect_bytes("%chacha20-encrypt", arg(args, 0), heap)?;
    let nonce_bytes = collect_bytes("%chacha20-encrypt", arg(args, 1), heap)?;
    let plaintext = collect_bytes("%chacha20-encrypt", arg(args, 2), heap)?;
    if key_bytes.len() != 32 {
        return Err(LispError::runtime(format!(
            "%chacha20-encrypt: key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    if nonce_bytes.len() != 12 {
        return Err(LispError::runtime(format!(
            "%chacha20-encrypt: nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| LispError::runtime(format!("%chacha20-encrypt: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|e| LispError::runtime(format!("%chacha20-encrypt: {e}")))?;
    let vals: Vec<Value> = ciphertext.iter().map(|&b| Value::Int(b as i64)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(%chacha20-decrypt key-bytes nonce-bytes ciphertext-bytes)` — authenticated
/// decryption (ChaCha20-Poly1305). Returns the plaintext as a byte vector, or
/// `:error` if the authentication tag fails (tampered or wrong key/nonce).
fn chacha20_decrypt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
    let key_bytes = collect_bytes("%chacha20-decrypt", arg(args, 0), heap)?;
    let nonce_bytes = collect_bytes("%chacha20-decrypt", arg(args, 1), heap)?;
    let ciphertext = collect_bytes("%chacha20-decrypt", arg(args, 2), heap)?;
    if key_bytes.len() != 32 {
        return Err(LispError::runtime(format!(
            "%chacha20-decrypt: key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    if nonce_bytes.len() != 12 {
        return Err(LispError::runtime(format!(
            "%chacha20-decrypt: nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| LispError::runtime(format!("%chacha20-decrypt: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.decrypt(nonce, ciphertext.as_slice()) {
        Ok(plaintext) => {
            let vals: Vec<Value> = plaintext.iter().map(|&b| Value::Int(b as i64)).collect();
            Ok(heap.alloc_vector(vals))
        }
        Err(_) => Ok(Value::Keyword(value::intern("error"))),
    }
}

/// `(%pbkdf2-sha256 password salt iterations key-len)` — derive a key from a
/// password using PBKDF2-HMAC-SHA256 (RFC 2898). Returns a byte vector of
/// `key-len` bytes. Use `iterations` ≥ 600,000 for password storage
/// (NIST SP 800-132 2023). Implemented over the `hmac` + `sha2` crates.
fn pbkdf2_sha256_fn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let password = expect_string(heap, "%pbkdf2-sha256", arg(args, 0))?;
    let salt = expect_string(heap, "%pbkdf2-sha256", arg(args, 1))?;
    let iterations = expect_int(heap, "%pbkdf2-sha256", arg(args, 2))?;
    let key_len = expect_int(heap, "%pbkdf2-sha256", arg(args, 3))?;
    if iterations <= 0 {
        return Err(LispError::runtime(
            "%pbkdf2-sha256: iterations must be positive",
        ));
    }
    if !(1..=512).contains(&key_len) {
        return Err(LispError::runtime(
            "%pbkdf2-sha256: key-len must be in 1..=512",
        ));
    }
    let hlen = 32usize; // SHA-256 output bytes
    let block_count = (key_len as usize + hlen - 1) / hlen;
    let pw = password.as_bytes();
    let mut dk = Vec::with_capacity(key_len as usize);
    for i in 1u32..=(block_count as u32) {
        // U_1 = HMAC(password, salt || INT(i))
        let mut mac = HmacSha256::new_from_slice(pw)
            .map_err(|e| LispError::runtime(format!("%pbkdf2-sha256: {e}")))?;
        mac.update(salt.as_bytes());
        mac.update(&i.to_be_bytes());
        let mut u: Vec<u8> = mac.finalize().into_bytes().to_vec();
        let mut t = u.clone();
        // U_n = HMAC(password, U_{n-1}); T_i = XOR of all U_j
        for _ in 1..(iterations as u32) {
            let mut mac2 = HmacSha256::new_from_slice(pw)
                .map_err(|e| LispError::runtime(format!("%pbkdf2-sha256: {e}")))?;
            mac2.update(&u);
            u = mac2.finalize().into_bytes().to_vec();
            for j in 0..hlen {
                t[j] ^= u[j];
            }
        }
        dk.extend_from_slice(&t);
    }
    dk.truncate(key_len as usize);
    let vals: Vec<Value> = dk.iter().map(|&b| Value::Int(b as i64)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(%git-resolve-ref url ref)` — resolve `ref` (a tag, branch, or commit) at the
/// remote `url` to a full commit hash via `git ls-remote`, or `nil` if no such
/// ref exists. For an annotated tag, prefers the peeled `^{}` line (the commit the
/// tag points to). When `ref` is already a commit SHA the remote doesn't advertise
/// (ls-remote returns nothing), it's returned as-is — a commit pins itself.
/// The package manager's ref-pinning mechanism (ADR-037); pinning policy is Brood.
fn git_resolve_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-resolve-ref", arg(args, 0))?;
    let r = expect_string(heap, "%git-resolve-ref", arg(args, 1))?;
    let out = run_git(&["ls-remote", &url, &r], None)?;
    if !out.status.success() {
        return Err(LispError::runtime(format!(
            "%git-resolve-ref: git ls-remote {} {} failed: {}",
            url,
            r,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut first: Option<&str> = None;
    let mut peeled: Option<&str> = None;
    for line in stdout.lines() {
        let sha = line.split_whitespace().next();
        if first.is_none() {
            first = sha;
        }
        if line.trim_end().ends_with("^{}") {
            peeled = sha;
        }
    }
    if let Some(s) = peeled.or(first) {
        return Ok(heap.alloc_string(s));
    }
    // No advertised ref: if `ref` itself looks like a commit SHA, it pins itself.
    let looks_like_sha = r.len() >= 7 && r.len() <= 40 && r.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_sha {
        Ok(heap.alloc_string(&r))
    } else {
        Ok(Value::Nil)
    }
}

/// `(%git-clone url dest ref commit)` — populate `dest` with a shallow clone of
/// `url` checked out at the exact `commit` (detached HEAD). Tries to fetch the
/// commit directly (servers that allow SHA-in-want, e.g. GitHub); falls back to
/// fetching `ref` then checking out `commit`. Returns `:ok`, or throws with git's
/// stderr. The package manager's fetch mechanism (ADR-037); the cache layout and
/// when-to-reclone policy are Brood (std/package.blsp).
fn git_clone(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-clone", arg(args, 0))?;
    let dest = expect_string(heap, "%git-clone", arg(args, 1))?;
    let gref = expect_string(heap, "%git-clone", arg(args, 2))?;
    let commit = expect_string(heap, "%git-clone", arg(args, 3))?;

    if let Some(parent) = std::path::Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LispError::runtime(format!(
                    "%git-clone: cannot create {}: {}",
                    parent.display(),
                    e
                ))
                .with_code(crate::error::error_codes::FILE_IO)
            })?;
        }
    }

    git_or_err(&["init", "-q", &dest], None)?;
    git_or_err(&["-C", &dest, "remote", "add", "origin", &url], None)?;

    // Fast path: fetch the exact commit shallowly. Many servers (GitHub) allow it.
    let direct = run_git(
        &[
            "-C", &dest, "fetch", "-q", "--depth", "1", "origin", &commit,
        ],
        None,
    )?;
    if !direct.status.success() {
        // Fallback: fetch the named ref (shallow first, then full if the server
        // rejects a shallow ref fetch), which must contain the locked commit.
        if git_or_err(
            &["-C", &dest, "fetch", "-q", "--depth", "1", "origin", &gref],
            None,
        )
        .is_err()
        {
            git_or_err(&["-C", &dest, "fetch", "-q", "origin", &gref], None)?;
        }
    }

    if git_or_err(&["-C", &dest, "checkout", "-q", "--detach", &commit], None).is_err() {
        return Err(LispError::runtime(format!(
            "%git-clone: commit {} is not reachable from {} at {}",
            commit, gref, url
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
        .with_hint("the ref may have moved since it was locked — try `nest update`"));
    }
    Ok(crate::core::value::kw("ok"))
}

/// `(%rm-rf path)` — recursively delete `path`. **Bounded to `_deps/`**: refuses
/// any path without a `_deps` component, so a mis-computed cache path can't delete
/// something outside the package cache. Idempotent (`:ok` if already absent). The
/// package manager's cache-eviction mechanism (ADR-037); `nest update` re-clones.
fn rm_rf(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%rm-rf", arg(args, 0))?;
    let under_deps = std::path::Path::new(&path)
        .components()
        .any(|c| c.as_os_str() == "_deps");
    if !under_deps {
        return Err(LispError::runtime(format!(
            "%rm-rf: refusing to delete {} — only paths under _deps/ may be removed",
            path
        ))
        .with_code(crate::error::error_codes::FILE_IO));
    }
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(crate::core::value::kw("ok")),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(crate::core::value::kw("ok")),
        Err(e) => Err(LispError::runtime(format!("%rm-rf: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(read-line)` — read one line from stdin, returning it as a string with the
/// trailing newline stripped, or `nil` at end of input (EOF / Ctrl-D). The one
/// irreducible I/O mechanism the Brood-hosted REPL (`std/repl.blsp`) can't
/// bootstrap; line *editing* on a TTY comes free from the terminal's cooked
/// mode, so this stays a plain blocking read.
fn read_line(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::BufRead;
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).map_err(|e| {
        LispError::runtime(format!("read-line: {}", e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    if n == 0 {
        return Ok(Value::Nil); // EOF
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(heap.alloc_string(&line))
}

/// `(slurp path)` — read the whole file at `path` and return it as a string. The
/// read-side counterpart to `spit`; unlike `load` it does not evaluate, so the
/// doc tooling can inspect a module's source (e.g. its leading docstring form).
fn slurp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "slurp", arg(args, 0))?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("slurp: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(heap.alloc_string(&content))
}

/// `(file-size path)` — the size of `path` in bytes, or nil if it's missing.
/// GC-safe: the arg is copied to an owned `String` up front and the result is a
/// scalar — no `Value` handle is held across an allocation or eval (and a builtin
/// never fires GC mid-execution; see `docs/memory-model.md`).
fn file_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-size", arg(args, 0))?;
    match std::fs::metadata(&path) {
        Ok(meta) => Ok(Value::Int(meta.len() as i64)),
        Err(_) => Ok(Value::Nil),
    }
}

/// `(delete-file path)` — remove the file at `path`. Idempotent (nil if already
/// absent); errors on a real I/O failure (e.g. it's a directory, or permission).
fn delete_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "delete-file", arg(args, 0))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::Nil),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Nil),
        Err(e) => Err(LispError::runtime(format!("delete-file: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(delete-dir path)` — remove a directory and everything under it. The
/// recursive sibling of `delete-file`; idempotent (nil if already absent),
/// errors on a real I/O failure. The mechanism behind test-fixture teardown.
fn delete_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "delete-dir", arg(args, 0))?;
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(Value::Nil),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Nil),
        Err(e) => Err(LispError::runtime(format!("delete-dir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(rename-file from to)` — rename/move `from` to `to` (replacing `to` if it
/// exists, per the platform). Returns nil; errors on failure.
fn rename_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "rename-file", arg(args, 0))?;
    let to = expect_string(heap, "rename-file", arg(args, 1))?;
    std::fs::rename(&from, &to).map_err(|e| {
        LispError::runtime(format!("rename-file: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

/// `(copy-file from to)` — copy the file `from` to `to` (replacing `to` if it
/// exists), preserving the contents byte-for-byte and the permission bits.
/// Returns nil; errors on failure. The binary-safe counterpart to a `slurp`+`spit`
/// (which is UTF-8 string I/O and would corrupt non-text files / drop the mode).
fn copy_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "copy-file", arg(args, 0))?;
    let to = expect_string(heap, "copy-file", arg(args, 1))?;
    std::fs::copy(&from, &to).map_err(|e| {
        LispError::runtime(format!("copy-file: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Nil)
}

/// `(file-mtime path)` — last-modified time of `path` as epoch-milliseconds, or
/// `nil` if the file is missing or its mtime can't be read. A cheap `stat`, not a
/// read — pairs with `load` to drive a hot-reloader: poll `file-mtime`, reload
/// only when it changes. Resolution is platform-dependent (typically nanoseconds
/// on Linux, truncated to ms here).
fn file_mtime(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-mtime", arg(args, 0))?;
    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(Value::Nil);
    };
    let Ok(modified) = meta.modified() else {
        return Ok(Value::Nil);
    };
    let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return Ok(Value::Nil);
    };
    Ok(Value::Int(since.as_millis() as i64))
}

/// `(file-stat path)` — one `stat` for `path` as a map, or `nil` if it is missing.
/// Collapses the `dir?` / `file-size` / `file-mtime` trio (each its own syscall)
/// into a single metadata read — the shape a directory lister (dired) wants per
/// entry. `:symlink?` and `:mode` describe the link itself (`symlink_metadata`),
/// while `:dir?` / `:size` / `:mtime` follow it (a symlink to a directory reports
/// `:dir? true` so it's navigable, yet `:symlink? true` so it can be marked). Off
/// unix there are no permission bits, so `:mode` is 0 and `:exec?` is false.
fn file_stat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-stat", arg(args, 0))?;
    // lstat for the link's own nature; stat (follows) for size/mtime/dir?-of-target.
    let Ok(lmeta) = std::fs::symlink_metadata(&path) else {
        return Ok(Value::Nil);
    };
    let symlink = lmeta.file_type().is_symlink();
    // Follow the link for the navigable facts; fall back to the link itself for a
    // dangling symlink (so a broken link still lists rather than vanishing).
    let meta = std::fs::metadata(&path).unwrap_or(lmeta);

    let epoch_ms = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| Value::Int(d.as_millis() as i64))
            .unwrap_or(Value::Nil)
    };
    let mtime = epoch_ms(meta.modified());
    let atime = epoch_ms(meta.accessed());

    #[cfg(unix)]
    let (mode, exec, nlink, uid, gid) = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = meta.permissions().mode();
        (m as i64 & 0o7777, m & 0o111 != 0, meta.nlink() as i64, meta.uid(), meta.gid())
    };
    #[cfg(not(unix))]
    let (mode, exec, nlink, uid, gid) = (0_i64, false, 1_i64, 0_u32, 0_u32);

    let kw = |k: &'static str| Value::Keyword(value::intern(k));
    // Owner/group names (getpwuid/getgrgid), falling back to the numeric id as a string.
    let owner = uid_name(uid).unwrap_or_else(|| uid.to_string());
    let group = gid_name(gid).unwrap_or_else(|| gid.to_string());
    let owner_v = heap.alloc_string(&owner);
    let group_v = heap.alloc_string(&group);
    let pairs = vec![
        (kw("dir?"), Value::Bool(meta.is_dir())),
        (kw("size"), Value::Int(meta.len() as i64)),
        (kw("mtime"), mtime),
        (kw("atime"), atime),
        (kw("symlink?"), Value::Bool(symlink)),
        (kw("exec?"), Value::Bool(exec)),
        (kw("mode"), Value::Int(mode)),
        (kw("nlink"), Value::Int(nlink)),
        (kw("uid"), Value::Int(uid as i64)),
        (kw("gid"), Value::Int(gid as i64)),
        (kw("owner"), owner_v),
        (kw("group"), group_v),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// The user name for `uid` via `getpwuid`, or `None` if it doesn't resolve. The libc
/// call returns a pointer into a shared static buffer, so a process-wide lock serialises
/// our calls (Brood schedules green processes across OS threads); the name is copied out
/// before the lock drops. `None` off unix.
#[cfg(unix)]
fn uid_name(uid: u32) -> Option<String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap();
    unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_name).to_str().ok().map(|s| s.to_string())
    }
}

/// The group name for `gid` via `getgrgid` (see `uid_name` for the locking note).
#[cfg(unix)]
fn gid_name(gid: u32) -> Option<String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap();
    unsafe {
        let gr = libc::getgrgid(gid as libc::gid_t);
        if gr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*gr).gr_name).to_str().ok().map(|s| s.to_string())
    }
}

#[cfg(not(unix))]
fn uid_name(_uid: u32) -> Option<String> {
    None
}
#[cfg(not(unix))]
fn gid_name(_gid: u32) -> Option<String> {
    None
}

/// `(getenv name)` — the value of environment variable `name` as a string, or nil
/// if it is unset. Lets Brood locate things like the user config directory.
fn getenv(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "getenv", arg(args, 0))?;
    match std::env::var(&name) {
        Ok(val) => Ok(heap.alloc_string(&val)),
        Err(_) => Ok(Value::Nil),
    }
}

/// `(hostname)` — this machine's short hostname (no domain), used to qualify a
/// node name as `name@host` (ADR-073). Reads `/proc/sys/kernel/hostname`,
/// falling back to `$HOSTNAME` then `"localhost"` — never errors, since a node
/// must always get *some* identity. Long/FQDN names are had by passing an
/// already-qualified name to `node-start` (`:foo@my.fqdn`), so we don't resolve
/// the FQDN here.
fn hostname(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let h = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "localhost".to_string());
    Ok(heap.alloc_string(&h))
}

/// `(%env-all)` — all environment variables as a `{string → string}` map.
fn env_all(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let pairs: Vec<(Value, Value)> = env
        .iter()
        .map(|(k, v)| (heap.alloc_string(k), heap.alloc_string(v)))
        .collect();
    Ok(heap.map_from_pairs(pairs))
}

/// `(%argv)` — command-line arguments as a vector of strings, including argv[0].
fn argv_builtin(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let args: Vec<String> = std::env::args().collect();
    let vals: Vec<Value> = args.iter().map(|a| heap.alloc_string(a)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(%os-type)` — the current OS as a keyword: `:linux`, `:macos`, or `:windows`.
fn os_type_builtin(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    #[cfg(target_os = "linux")]
    return Ok(Value::Keyword(value::intern("linux")));
    #[cfg(target_os = "macos")]
    return Ok(Value::Keyword(value::intern("macos")));
    #[cfg(target_os = "windows")]
    return Ok(Value::Keyword(value::intern("windows")));
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return Ok(Value::Keyword(value::intern("unknown")));
}

/// `(%os-cmd prog args)` — run `prog` with `args` (list or vector of strings),
/// capturing stdout and stderr. Returns `{:stdout s :stderr s :exit n}`.
fn os_cmd(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "%os-cmd", arg(args, 0))?;
    let mut cmd = std::process::Command::new(&prog);
    if args.len() > 1 {
        let raw = heap.seq_items(arg(args, 1))?;
        for a in &raw {
            cmd.arg(expect_string(heap, "%os-cmd", *a)?);
        }
    }
    let output = cmd.output().map_err(|e| {
        LispError::runtime(format!("%os-cmd: {prog}: {e}"))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    })?;
    let stdout = heap.alloc_string(&String::from_utf8_lossy(&output.stdout));
    let stderr = heap.alloc_string(&String::from_utf8_lossy(&output.stderr));
    let exit_code = output.status.code().unwrap_or(-1) as i64;
    let kw = |k: &'static str| Value::Keyword(value::intern(k));
    Ok(heap.map_from_pairs(vec![
        (kw("stdout"), stdout),
        (kw("stderr"), stderr),
        (kw("exit"), Value::Int(exit_code)),
    ]))
}

/// `(%halt code)` — terminate the process immediately with `code`.
fn halt_builtin(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let code = expect_int(heap, "%halt", arg(args, 0))?;
    std::process::exit(code as i32);
}

/// `(run-process prog args)` — run external program `prog` with `args` (a list or
/// vector of strings), inheriting stdio, and return its exit code as an integer
/// (-1 if killed by a signal). The Emacs `call-process` analogue: the general
/// subprocess mechanism (used by the project scaffolder's `git init`).
fn run_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let prog = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "run-process",
                "string program",
                pv,
            ))
        }
    };
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        match a {
            Value::Str(id) => argv.push(heap.string(id).to_string()),
            _ => {
                return Err(LispError::type_err(
                    "run-process: arguments must be strings",
                ))
            }
        }
    }
    match std::process::Command::new(&prog).args(&argv).status() {
        Ok(status) => Ok(Value::Int(status.code().unwrap_or(-1) as i64)),
        Err(e) => Err(LispError::runtime(format!("run-process: {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("check that the program is on PATH and the args are well-formed")),
    }
}

fn apply_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity(
            "apply: expected a function and an argument list",
        ));
    }
    // Bind `last` after the guard so the slice indexing below is robust to
    // refactors of the guard: anyone moving / tightening it can't accidentally
    // leave a bare `args[args.len() - 1]` indexing into an empty slice.
    let last = args.len() - 1;
    let f = args[0];
    let mut argv = args[1..last].to_vec();
    argv.extend(heap.seq_items(args[last])?);
    // Run the target through the active engine (the VM when on), so `apply`-as-a-value
    // — `(map apply …)`, `(reduce apply …)`, apply stored in data — runs its callee
    // compiled, consistent with a direct `(apply f …)` call. This is safe against the
    // `(apply f …)`-driven tail recursion that once forced the tree-walker here
    // (`apply_tail_recursion_does_not_overflow`): a **direct** `apply` call is unfolded
    // by the VM's `dispatch` (it matches the resolved callee, so even `apply` bound to
    // another name unfolds) and TCO'd by the driver, so it never reaches this native;
    // `apply_builtin` is now only hit when a *native* HOF invokes `apply` per element,
    // which loops rather than tail-recurses — one `apply_engine` frame per call, never
    // accumulating. (Deep non-tail recursion in the callee is bounded by the VM's
    // `MAX_BC_FRAMES` guard, not the native stack.)
    apply_engine(heap, f, &argv, env)
}

// ---------- macros ----------

fn macroexpand_1(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let (expanded, _) = crate::eval::macros::macroexpand_1(heap, arg(args, 0), env)?;
    Ok(expanded)
}
// `macroexpand` is now a Brood prelude fn over `macroexpand-1` (ADR-064).

/// `(check 'form)` — run the advisory type checker over `form` (macro-expanded
/// first, like the real compile pass) and return a list of warning strings, or
/// `nil` when nothing is provably wrong. Advisory only: it never raises.
fn check_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    let warnings = crate::types::check::check_form(heap, form);
    let mut out = Vec::with_capacity(warnings.len());
    for w in &warnings {
        out.push(heap.alloc_string(w));
    }
    Ok(heap.list(out))
}

/// `(check-file path)` — run the advisory type checker over every top-level
/// form in the file at `path` and return a list of pre-formatted warning
/// strings (each `"path:line:col: warning: message"`), or `nil` if clean.
///
/// Reads but does **not** evaluate the file — same `check_file` walk the
/// `brood --check` CLI uses, with the file-globals accumulator threaded
/// across top-level forms. The whole-file-at-once shape is what lets `(defn
/// foo …)` at line 1 silence the unbound check on `(foo …)` at line 100. Used
/// by `(check-project)` in `std/project.blsp` for the `nest test` / `nest run`
/// pre-flight.
fn check_file_builtin(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("check-file: cannot read {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos, msg) in &warnings {
        let s = match pos {
            Some(p) => format!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
            None => format!("{}: warning: {}", path, msg),
        };
        out.push(heap.alloc_string(&s));
    }
    Ok(heap.list(out))
}

/// `(check-file-structured path)` — the data-shaped counterpart of
/// `check-file`. Returns a list of `{:file :line :col :message}` maps (or
/// `{:file :message}` for warnings without a position — the advisory
/// checker doesn't carry spans through macroexpansion yet, ADR-024). Used
/// by the `nest mcp` `check` tool (step 1c-a) and any other consumer that
/// wants structured diagnostics rather than a GNU-line string to re-parse.
fn check_file_structured(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file-structured", arg(args, 0))?;
    let src = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!(
            "check-file-structured: cannot read {}: {}",
            path, e
        ))
        .with_code(crate::error::error_codes::FILE_IO)
    })?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
    let file_kw = Value::Keyword(value::intern("file"));
    let line_kw = Value::Keyword(value::intern("line"));
    let col_kw = Value::Keyword(value::intern("col"));
    let msg_kw = Value::Keyword(value::intern("message"));
    let file_val = heap.alloc_string(&path);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos_opt, msg) in &warnings {
        let msg_val = heap.alloc_string(msg);
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(4);
        entries.push((file_kw, file_val));
        if let Some(p) = pos_opt {
            entries.push((line_kw, Value::Int(p.line as i64)));
            entries.push((col_kw, Value::Int(p.col as i64)));
        }
        entries.push((msg_kw, msg_val));
        out.push(heap.map_from_pairs(entries));
    }
    Ok(heap.list(out))
}

/// `(check-string-structured src)` — the source-string counterpart of
/// `check-file-structured`: advisory type-check the Brood source string `src` and
/// return a list of `{:line :col :message}` maps (1-based positions; no `:file`).
/// Returns `()` when `src` doesn't parse — e.g. incomplete input while an editor
/// buffer is mid-edit — so a live diagnostics loop never errors on an unbalanced
/// buffer; warnings reappear once it parses. Reuses the same checker as the file
/// variant (`types::check::check_file`).
fn check_string_structured(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "check-string-structured", arg(args, 0))?;
    let forms = match reader::read_all_positioned(heap, &src) {
        Ok(fs) => fs,
        // unparsable (e.g. mid-edit) — no diagnostics rather than an error
        Err(_) => return Ok(heap.list(Vec::new())),
    };
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
    let line_kw = Value::Keyword(value::intern("line"));
    let col_kw = Value::Keyword(value::intern("col"));
    let msg_kw = Value::Keyword(value::intern("message"));
    let mut out = Vec::with_capacity(warnings.len());
    for (pos_opt, msg) in &warnings {
        let msg_val = heap.alloc_string(msg);
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(3);
        if let Some(p) = pos_opt {
            entries.push((line_kw, Value::Int(p.line as i64)));
            entries.push((col_kw, Value::Int(p.col as i64)));
        }
        entries.push((msg_kw, msg_val));
        out.push(heap.map_from_pairs(entries));
    }
    Ok(heap.list(out))
}

// ---------- source positions (editor tooling; see docs/tooling.md) ----------

/// `(form-pos form)` — the `[line col]` (1-based) where `form` was read, or
/// `nil`. Recorded by the reader for list forms; used by the test macros to
/// capture a test's source line *before* the form expands.
fn form_pos(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.form_pos(arg(args, 0)) {
        Some(p) => Ok(heap.alloc_vector(vec![Value::Int(p.line as i64), Value::Int(p.col as i64)])),
        None => Ok(Value::Nil),
    }
}

/// `(current-file)` — the path of the file currently being `load`ed, or `nil`
/// (e.g. at the REPL). Maintained by `load`.
/// `(source-location 'name)` — where `name`'s global definition was loaded from,
/// as `[file line col]`, or `nil` if it has no recorded site (a Rust builtin, or
/// an unknown/local name). Prelude globals resolve to a materialized copy of the
/// standard library. The site is captured at load time
/// before macroexpansion, so `defn`/`defmacro` definitions are located
/// accurately. The image-query foundation for cross-file goto-definition (ADR-031
/// / docs/lsp.md). Takes a symbol, so quote it: `(source-location 'foo)`.
fn source_location(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "source-location",
                "symbol",
                other,
            ))
        }
    };
    match heap.def_site(name) {
        Some(loc) => {
            let file = heap.alloc_string(&loc.file);
            Ok(heap.alloc_vector(vec![
                file,
                Value::Int(loc.pos.line as i64),
                Value::Int(loc.pos.col as i64),
            ]))
        }
        None => Ok(Value::Nil),
    }
}

/// `(references-in-source name source)` — every occurrence of the global `name`
/// in `source`, as a list of `[line col]` (both 1-based), in document order. A
/// local that shadows the name is excluded. Pure: it parses the string and
/// holds no project state, so the Brood-side `callers` MCP tool maps it over a
/// project's files for cross-file find-references (ADR-031 §Cross-file,
/// docs/lsp.md). `name` may be a symbol or a string.
fn references_in_source(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "references-in-source",
                "symbol or string",
                other,
            ))
        }
    };
    let src = expect_string(heap, "references-in-source", arg(args, 1))?;
    let root = cst::parse(&src);
    let tree = crate::syntax::scope::analyze(&root, &src);
    let starts = line_starts(&src);
    let occ: Vec<Value> = tree
        .references_to_global(&root, &src, &name)
        .into_iter()
        .map(|span| {
            let (line, col) = line_col(&src, &starts, span.start as usize);
            heap.alloc_vector(vec![Value::Int(line as i64), Value::Int(col as i64)])
        })
        .collect();
    Ok(heap.list(occ))
}

/// Byte offsets of each line start in `src` (line 0 begins at 0). Built once so
/// repeated byte→line/col lookups in one source are cheap.
fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// 1-based (line, col) of byte offset `b`, col counted in characters. `b` must
/// be a char boundary (CST spans always are).
fn line_col(src: &str, starts: &[usize], b: usize) -> (u32, u32) {
    let line = starts.partition_point(|&s| s <= b) - 1; // 0-based
    let col = src[starts[line]..b].chars().count();
    (line as u32 + 1, col as u32 + 1)
}

fn current_file(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.current_file().map(str::to_string) {
        Some(f) => Ok(heap.alloc_string(&f)),
        None => Ok(Value::Nil),
    }
}

// ---------- introspection (editor tooling; see docs/lsp.md) ----------

/// `(doc f)` — the docstring of a function or macro value, or `nil`. A docstring
/// is the leading string literal in a `fn`/`defn` body (stored on the closure
/// when more body follows it). Powers hover / `describe-function`.
fn doc(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let text = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => heap.closure(id).doc.clone(),
        // A primitive's docstring lives on the `NativeFn` (the `PRIMITIVE_DOCS`
        // table), since it has no Brood body to carry a leading string.
        Value::Native(id) => {
            let d = heap.native(id).doc;
            (!d.is_empty()).then(|| d.to_string())
        }
        _ => None,
    };
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::Nil),
    }
}

/// `(arglist f)` — the parameter list of a function, macro, or primitive as a
/// list, mirroring the source surface: required names, then `&optional` names,
/// then `& rest`. `nil` for a non-function (or a primitive without recorded
/// params). Feeds signature help / hover.
fn arglist(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => id,
        // A primitive carries its param names as a flat `&'static` list (incl. any
        // `&`/`&optional` markers, already in order) — hand them back as symbols.
        Value::Native(id) => {
            let params = heap.native(id).params;
            if params.is_empty() {
                return Ok(Value::Nil);
            }
            let items: Vec<Value> = params.iter().map(|p| value::sym(p)).collect();
            return Ok(heap.list(items));
        }
        _ => return Ok(Value::Nil),
    };
    // Copy the parts out before re-borrowing the heap mutably to build the list.
    // For a multi-arity closure there's no single arglist; show the last clause
    // (conventionally the most general — e.g. the variadic `(a b & more)`).
    let (params, optionals, rest) = {
        let cl = heap.closure(id);
        let arm = cl.arms.last().expect("closure has at least one arm");
        (
            arm.params.clone(),
            arm.optionals.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
            arm.rest,
        )
    };
    let mut items: Vec<Value> = params.into_iter().map(Value::Sym).collect();
    if !optionals.is_empty() {
        items.push(value::sym("&optional"));
        items.extend(optionals.into_iter().map(Value::Sym));
    }
    if let Some(r) = rest {
        items.push(value::sym("&"));
        items.push(Value::Sym(r));
    }
    Ok(heap.list(items))
}

/// `(global-names)` — a list of every symbol bound in the global table
/// (prelude + user `def`s), sorted by spelling so the order is deterministic
/// (for completion / workspace-symbol tooling and reproducible doc generation).
/// Special forms and the core control/binding macros — the keyword-like heads:
/// the single source of truth for "what reads as a keyword". Read from Brood via
/// the `(special-forms)` primitive (so `std/highlight.blsp` highlights from this
/// list) and from the LSP (`semantic_tokens` / `completion` import it rather than
/// keeping a copy), so the runtime and the tooling can't drift. Mirrors
/// `brood.el`'s `brood-special-forms` plus the `def`-family heads.
pub const SPECIAL_FORMS: &[&str] = &[
    kw::IF,
    kw::DO,
    kw::DEF,
    kw::FN,
    kw::LAMBDA,
    kw::LET,
    kw::LET_STAR,
    kw::LETREC,
    kw::QUOTE,
    kw::QUASIQUOTE,
    kw::DEFMACRO,
    kw::DEFN,
    kw::DEFDYN,
    kw::DEFMODULE,
    kw::WHEN,
    kw::UNLESS,
    kw::COND,
    kw::AND,
    kw::OR,
    kw::MATCH,
    kw::MATCH_STAR,
    kw::TRY,
    kw::CATCH,
    kw::THROW,
    kw::RECEIVE,
    kw::BINDING,
    kw::DOLIST,
    kw::DOSEQ,
    kw::DOTIMES,
    kw::FOR,
    kw::THREAD_FIRST,
    kw::THREAD_LAST,
    // Core macros (std/prelude.blsp) that read as keywords — highlight-only, not
    // evaluator special forms (ADR-092). Promoted here so every editor (VS Code via
    // `nest grammar`, Emacs, the REPL highlighter) + the LSP colour them from one
    // source. `throw`/`receive` are already above (they're in the core set).
    kw::SPAWN,
    kw::SPAWN_LINK,
    kw::REMOTE_SPAWN,
    kw::REMOTE_SPAWN_SYNC,
    kw::ERROR,
    kw::WITH_OUT_STR,
    kw::BENCH,
];

/// `(special-forms)` — the list of special-form / core-macro names (strings) that
/// read as keywords, for tooling (the highlighter, completion). Returns the
/// canonical `SPECIAL_FORMS`, so Brood and the LSP share one list.
fn special_forms(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = SPECIAL_FORMS.iter().map(|s| heap.alloc_string(s)).collect();
    Ok(heap.list(items))
}

fn global_names(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let mut syms = heap.global_symbols();
    // `symbol_name` locks the interner and allocates, so resolve each spelling
    // once (cached) rather than twice per comparison.
    syms.sort_by_cached_key(|&s| value::symbol_name(s));
    let syms: Vec<Value> = syms.into_iter().map(Value::Sym).collect();
    Ok(heap.list(syms))
}

/// `(bound? 'name)` — whether `name` is bound in the current scope (which
/// reaches the global table). Takes a symbol, so quote it: `(bound? 'foo)`.
fn bound_p(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Sym(s) => Ok(Value::Bool(heap.env_get(env, s).is_some())),
        other => Err(LispError::wrong_type(heap, "bound?", "symbol", other)),
    }
}

fn gensym(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(heap, other),
    };
    Ok(value::gensym(&prefix))
}

// ---------- errors / control ----------

/// `(%make-macro f)` — tag the closure `f` as a macro: the expander calls it on
/// the *unexpanded* argument forms and splices the result in place of the call.
/// The `defmacro` macro (std/prelude.blsp) lowers to this, so macro definition is
/// plain Brood over a one-line primitive rather than its own core special form.
fn make_macro(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Fn(id) => Ok(Value::Macro(id)),
        other => Err(LispError::type_err(format!(
            "%make-macro: expected a fn, got {}",
            value::tag(other).name()
        ))),
    }
}

fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Err(LispError::thrown(arg(args, 0), heap))
}

/// `(%force-panic [msg])` — debug-only. Deliberately panics from a primitive,
/// so tests can exercise the host-side `catch_unwind` boundary (currently the
/// MCP server's `call_tool`). Not a Brood-clean error path — this *is* a Rust
/// `panic!`; if no host catches it, the process dies. There's no Brood
/// reason to call this outside the regression test.
#[cfg(debug_assertions)]
fn force_panic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let msg = match args.first() {
        Some(Value::Str(id)) => heap.string(*id).to_string(),
        Some(other) => printer::display(heap, *other),
        None => "%force-panic invoked (no message)".to_string(),
    };
    panic!("{}", msg);
}

/// `(%blob-ptr s)` — debug-only. The raw `SharedBlob` address backing `s`,
/// as an integer (for identity comparison across processes). `nil` for
/// inline (small) strings and PRELUDE/RUNTIME handles.
#[cfg(debug_assertions)]
fn blob_ptr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_ptr(id)
            .map(|p| Value::Int(p as i64))
            .unwrap_or(Value::Nil)),
        other => Err(LispError::type_err(format!(
            "%blob-ptr: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

/// `(%blob-strong-count s)` — debug-only. Current `Arc::strong_count` for
/// the `SharedBlob` backing `s`. `nil` for inline / non-LOCAL strings.
/// Approximate under live concurrent senders/receivers (the count moves);
/// stable when callers are quiescent (what the leak-check test asserts).
#[cfg(debug_assertions)]
fn blob_strong_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(heap
            .local_shared_blob_strong_count(id)
            .map(|n| Value::Int(n as i64))
            .unwrap_or(Value::Nil)),
        other => Err(LispError::type_err(format!(
            "%blob-strong-count: expected a string, got {}",
            value::tag(other).name()
        ))),
    }
}

// ---------- processes ----------

fn spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pid = crate::process::spawn(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
}

/// `(%spawn-named name thunk)` — idempotent named spawn. If `name` (a
/// keyword or symbol) is currently registered to a still-alive pid, return
/// that pid and **do not** spawn — `thunk` is never evaluated. Otherwise,
/// drop any stale registration, spawn the thunk as a new green process,
/// register it under `name`, and return the new pid.
///
/// The check-or-spawn step is atomic under `NAMES`'s write lock — two
/// concurrent `(spawn :name …)` calls can't both spawn; the loser sees
/// the winner's pid. The user-facing `(spawn name expr)` macro wraps an
/// expression into a thunk the same way `(spawn expr)` does, so the
/// expression's free locals are captured lexically (ADR-033).
fn spawn_named(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) | Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%spawn-named",
                "keyword or symbol",
                v,
            ))
        }
    };
    let thunk = arg(args, 1);
    if !matches!(thunk, Value::Fn(_)) {
        return Err(LispError::wrong_type(
            heap,
            "%spawn-named",
            "function",
            thunk,
        ));
    }
    // `spawn_or_get`'s spawner is fallible — `?` propagates a real
    // `LispError` if `process::spawn` rejects the thunk (defensive: with the
    // `Value::Fn(_)` type-check above, that shouldn't fire today, but a
    // future change to `promote`/`spawn` won't silently panic).
    let pid = crate::dist::spawn_or_get(name, || crate::process::spawn(heap, thunk))?;
    Ok(crate::process::pid_value(pid))
}

fn send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send(heap, arg(args, 0), arg(args, 1))?;
    Ok(Value::Nil)
}

/// `(exit pid reason)` — send an exit signal to a local green process (Erlang
/// `exit/2`). `reason = :kill` is the untrappable hard kill (dies at its next
/// reduction tick, or now if parked); any other reason is the soft signal (dies at
/// its next `receive`). Returns nil. A no-op for a dead/unknown pid.
fn exit_proc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let reason = crate::process::to_message(heap, arg(args, 1))?;
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::exit(id, reason);
            Ok(Value::Nil)
        }
        // Cross-node exit (ADR-077): ship a non-link `Frame::Exit` routed to the
        // peer's `scheduler::exit` (kill-style, like the local path).
        Value::Pid { node, id } => {
            crate::dist::exit_remote(node, id, reason);
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err("exit: first argument must be a pid")),
    }
}

/// `(link pid)` — symmetrically link the current process and `pid`, local or
/// remote (ADR-077). A cross-node link ships a `Frame::Link`; either side's death
/// reaches the other, and a net-split fires `:noconnection`. Returns nil.
fn link_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::link_self(id);
            Ok(Value::Nil)
        }
        Value::Pid { node, id } => {
            crate::dist::link_remote(node, id, crate::process::self_pid());
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err("link: argument must be a pid")),
    }
}

/// `(unlink pid)` — drop the link between the current process and `pid` (local or
/// remote).
fn unlink_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::unlink_self(id);
            Ok(Value::Nil)
        }
        Value::Pid { node, id } => {
            crate::dist::unlink_remote(node, id, crate::process::self_pid());
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err("unlink: argument must be a pid")),
    }
}

/// `(trap-exit on)` — set the current process's `trap_exit` flag; return the
/// previous value. Only `nil`/`false` are falsy (the language truthiness rule).
fn trap_exit_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let on = !matches!(arg(args, 0), Value::Nil | Value::Bool(false));
    let prev = crate::process::set_trap_exit(crate::process::self_pid(), on);
    Ok(Value::Bool(prev))
}

/// `(monitor pid)` — watch `pid`; returns a monitor `ref`. The caller receives
/// `[:down <ref> <pid> <reason>]` when `pid` dies (immediately, reason `:noproc`,
/// if it is already dead).
fn monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Local pid: in-process registration, returns a fresh mref.
            Ok(crate::process::monitor(id))
        }
        Value::Pid { node, id } => {
            // Remote pid: same shape — mint a mref, register *here* (so
            // demonitor can find it later, and net-split can fire
            // `:noconnection`), and ship a `Frame::Monitor` to the peer
            // which routes through the same `process::add_monitor` on the
            // far side.
            let mref = crate::process::next_ref();
            let watcher = crate::process::self_pid();
            crate::dist::monitor_remote(node, id, watcher, mref);
            Ok(Value::Ref(mref))
        }
        // `{:name n :node node}` address: resolve to a pid via `whereis` and
        // monitor that pid. Only the local-node case is supported — a remote
        // `{:name :node}` address has no protocol to resolve the name on the
        // far side at monitor time, so we redirect the user to ship the pid
        // directly. Documented in `docs/primitives.md`.
        Value::Map(mid) => {
            let (name, node) = crate::process::read_name_address(heap, mid)?;
            if crate::dist::is_local(node) {
                match crate::dist::whereis(name) {
                    Some(pid) => Ok(crate::process::monitor(pid)),
                    // Unregistered name: behave as if the pid were already
                    // dead — fire :noproc immediately. `process::monitor`
                    // already does this for an unknown local pid, so route
                    // through it with a fresh-but-dead id placeholder.
                    None => Ok(crate::process::monitor(u64::MAX)),
                }
            } else {
                Err(LispError::type_err(
                    "monitor: remote {:name :node} addresses aren't resolvable for monitor — pass the pid",
                ))
            }
        }
        _ => Err(LispError::type_err(
            "monitor: first argument must be a pid or a {:name :node} address",
        )),
    }
}

/// `(demonitor mref)` — drop the monitor created by `(monitor …)`. Tries the
/// local table first; if the mref isn't there it must have been on a remote
/// peer, so a `Frame::Demonitor` is fanned out to every connected peer that
/// holds a pending remote monitor with this watcher + mref.
fn demonitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(n) => {
            // Local first (in-process MONITORS table).
            crate::process::demonitor(n);
            // Then ask any peer holding this mref to drop their watcher.
            // We scan PENDING_REMOTE for matching entries and `Demonitor` each
            // unique peer once. The same `process::drop_monitor` predicate the
            // local demonitor used is reused on the far side via the frame
            // handler.
            crate::process::demonitor_remote_fanout(n);
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err(
            "demonitor: argument must be a monitor ref",
        )),
    }
}

/// `(%receive matcher timeout on-timeout)` — the selective-receive primitive the
/// `receive` macro (`std/prelude.blsp`) expands to. See `crate::process::receive_match`.
fn receive_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_match(heap, arg(args, 0), arg(args, 1), arg(args, 2))
}

fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(crate::process::pid_value(crate::process::self_pid()))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
fn make_ref(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Ref(crate::process::next_ref()))
}

// ----- distributed nodes -----------------------------------------------------

/// Coerce a node/name argument (a keyword or symbol) to its interned `Symbol`.
/// Goes through the same `wrong_type` formatter as the other `expect_*`
/// helpers — pre-fix this one used `type_err` and lost the offending value
/// from the message, the one expect-family inconsistency the review flagged.
fn expect_node_name(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "keyword or symbol",
        Value::Keyword(s) => s,
        Value::Sym(s) => s,
    )
}

/// `(node-start name "host:port" cookie)` — name this runtime and listen for peer
/// nodes. Returns the node name.
/// `(%node-listen name addr cookie)` — the listen mechanism behind the prelude's
/// `node-start`. `addr` carries the transport (`"unix:PATH"` / `"tcp:HOST:PORT"`);
/// the path/cookie/transport policy lives in `std/prelude.blsp` (ADR-068).
fn node_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "%node-listen", arg(args, 0))?;
    let addr = expect_string(heap, "%node-listen", arg(args, 1))?;
    let cookie = expect_string(heap, "%node-listen", arg(args, 2))?;
    crate::dist::node_listen(name, &addr, cookie).map_err(|e| {
        LispError::runtime(format!("node-start: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::Keyword(name))
}

/// `(%node-also-listen addr)` — add another listener to an already-started node
/// (dual-listen, ADR-074). `addr` carries the transport (`"unix:PATH"` /
/// `"tcp:HOST:PORT"`); shares the node's existing identity + cookie.
fn node_also_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let addr = expect_string(heap, "%node-also-listen", arg(args, 0))?;
    crate::dist::node_also_listen(&addr).map_err(|e| {
        LispError::runtime(format!("node-also-listen: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::Nil)
}

/// `(%node-connect peer addr)` — the dial mechanism behind the prelude's
/// `connect`. `peer` is the expected node name (self-guard + de-dup); `addr`
/// carries the transport. Returns the peer's authoritative node name.
fn node_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let peer = expect_node_name(heap, "%node-connect", arg(args, 0))?;
    let addr = expect_string(heap, "%node-connect", arg(args, 1))?;
    let real = crate::dist::node_connect(peer, &addr).map_err(|e| {
        LispError::runtime(format!("connect: {e}"))
            .with_code(crate::error::error_codes::DISTRIBUTION)
    })?;
    Ok(Value::Keyword(real))
}

/// `(random-token n)` — `n` cryptographically-strong random bytes from the OS
/// RNG, hex-encoded into a `2n`-char string. The CSPRNG is mechanism (Rust); the
/// node cookie's generation policy is Brood (`node-cookie`, ADR-068).
fn random_token(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "random-token", arg(args, 0))?;
    if !(0..=4096).contains(&n) {
        return Err(LispError::runtime(
            "random-token: byte count must be in 0..=4096",
        ));
    }
    let mut bytes = vec![0u8; n as usize];
    getrandom::fill(&mut bytes)
        .map_err(|e| LispError::runtime(format!("random-token: OS RNG unavailable: {e}")))?;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    Ok(heap.alloc_string(&s))
}

/// `(spit-private path s)` — write `s` to `path` with owner-only (`0600`)
/// permissions, creating the parent directory if needed. The private-by-default
/// write a secret needs (`spit` leaves a world-readable file); the cookie-file
/// policy that uses it is Brood (`node-cookie`, ADR-068).
fn spit_private(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = expect_string(heap, "spit-private", arg(args, 0))?;
    let content = expect_string(heap, "spit-private", arg(args, 1))?;
    let err = |e: std::io::Error| {
        LispError::runtime(format!("spit-private: {path}: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(err)?;
    // `.mode` only applies on *create*; enforce 0600 on a pre-existing file too.
    let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    f.write_all(content.as_bytes()).map_err(err)?;
    Ok(Value::Nil)
}

/// `(register name pid)` — bind a local name so peers can address this process by
/// `{:name name :node this-node}` before they hold its pid. Returns the pid.
fn register_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "register", arg(args, 0))?;
    match arg(args, 1) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::dist::register(name, id);
            Ok(Value::Pid { node, id })
        }
        Value::Pid { .. } => Err(LispError::type_err(
            "register: can only register a local pid",
        )),
        _ => Err(LispError::type_err(
            "register: second argument must be a pid",
        )),
    }
}

/// `(node-name)` — this runtime's node name (`:nonode` until `node-start`).
fn node_name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Keyword(crate::dist::local_node()))
}

/// `(whereis name)` — the **local** pid registered under `name`, or `nil`.
/// Lets idempotent bootstrap shapes test for "is this server already running
/// here?" before re-`spawn`ing — see `remote-spawn` in `std/prelude.blsp`.
/// A remote-side registration isn't visible here; this is a strictly local
/// lookup over the `NAMES` table.
fn whereis_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "whereis", arg(args, 0))?;
    match crate::dist::whereis(name) {
        Some(id) => Ok(Value::Pid {
            node: crate::dist::local_node(),
            id,
        }),
        None => Ok(Value::Nil),
    }
}

/// `(monitor-node name)` — the calling process is sent `[:nodedown name]` when a
/// link to `name` goes down (heartbeat timeout or clean close). Returns the name.
fn monitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "monitor-node", arg(args, 0))?;
    crate::dist::monitor_node(name, crate::process::self_pid());
    Ok(Value::Keyword(name))
}

/// `(demonitor-node name)` — cancel the calling process's node monitor for `name`.
/// A no-op if no monitor is registered. Returns `nil`.
fn demonitor_node(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "demonitor-node", arg(args, 0))?;
    crate::dist::demonitor_node(name, crate::process::self_pid());
    Ok(Value::Nil)
}

/// `(disconnect name)` — drop the link to peer `name` now (Erlang's
/// `disconnect_node`). Returns `true` if a link existed, `false` otherwise.
fn disconnect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "disconnect", arg(args, 0))?;
    Ok(Value::Bool(crate::dist::disconnect(name)))
}

/// `(nodes)` — a list of currently connected peer node names.
fn nodes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = crate::dist::connected_nodes()
        .into_iter()
        .map(Value::Keyword)
        .collect();
    Ok(heap.list(names))
}

/// `(spawn-count)` — how many green processes have been spawned since the program
/// started. (Green processes are cheap coroutines, not OS threads — step 4b.)
fn spawn_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::spawn_count() as i64))
}

/// `(peak-threads)` — high-water mark of processes running *simultaneously*
/// (bounded by the worker-pool size); how much parallelism was actually reached.
fn peak_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::peak_threads() as i64))
}

/// `(worker-threads)` — size of the scheduler's worker-thread pool that runs the
/// green processes (≈ `nproc`, or the `-j` setting); 0 until the first spawn.
fn worker_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::worker_threads() as i64))
}

/// `(steal-count)` — how many fresh processes the scheduler work-stole across
/// worker threads since program start. A diagnostic of how much the pool had to
/// rebalance; 0 means placement-at-spawn kept it even.
fn steal_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::steal_count() as i64))
}

/// `(list-processes)` — every currently-live local pid as a `Pid` value
/// (carrying this runtime's node identity, so the list is `send`-routable as
/// returned). Order is unspecified; sort by `.id` if you need stability.
/// Used by agents / the `nest mcp` `processes` tool to enumerate what's been
/// spawned in the session.
fn list_processes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = crate::process::list_local_pids()
        .into_iter()
        .map(crate::process::pid_value)
        .collect();
    Ok(heap.list(items))
}

/// `(%isolate thunk)` — call `thunk` (no args) with a *private copy* of the
/// runtime's global bindings: any `def` it makes is rolled back when it
/// returns, so it cannot affect other code. The test framework wraps each
/// `:isolated` test in this so a test's definitions never leak to another test.
/// Restores the bindings even if the thunk raises (the error then propagates).
///
/// This only isolates *bindings* — the shared code slabs and the symbol interner
/// still grow (memory, not behaviour; there's no GC yet) — and it is sound only
/// with no other process mutating globals concurrently, which the runner ensures
/// by running isolated tests alone.
fn isolate(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let saved = heap.snapshot_globals();
    // Pids alive before the run, to tell apart the ones the thunk spawns.
    let before: std::collections::HashSet<u64> =
        crate::process::list_local_pids().into_iter().collect();
    let result = apply_engine(heap, thunk, &[], env);
    // Reap processes the thunk spawned and left running, BEFORE the wholesale
    // global restore below. Otherwise an orphan still running the test's code (a
    // server it spawned but never stopped) looks up a global the test `def`'d,
    // finds it gone after the swap, and dies with a bogus `unbound symbol` (the
    // flaky-suite race). Kill the newcomers, then **yield** until they deregister
    // — `crate::process::yield_now`, NOT `std::thread::sleep`: this runs inside the
    // isolated unit's own green process, so a thread sleep would freeze its worker
    // and starve any orphan pinned to that same worker. Bounded so a wedged orphan
    // can't hang the run.
    let spawned: std::collections::HashSet<u64> = crate::process::list_local_pids()
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    if !spawned.is_empty() {
        let kill = crate::process::Message::Keyword(crate::core::value::intern(
            crate::process::keywords::KILL,
        ));
        for &pid in &spawned {
            // Unlink the child from THIS isolate runner before killing it. A child the
            // thunk `spawn-link`ed is symmetrically linked to us, so a bare
            // `(exit pid :kill)` would propagate `:killed` back through the link and
            // kill the runner itself — even though we're only cleaning up leftovers.
            // Dropping the link first lets the reap take down any straggler (e.g. a
            // server whose async `(stop …)` hasn't finished dying yet) without taking us
            // with it. Best-effort + a no-op for an unlinked child. (Fixes a capture-mode
            // flake where the stop-vs-reap race left a linked server alive at reap; §8.4.)
            crate::process::unlink_self(pid);
            crate::process::exit(pid, kill.clone());
        }
        for _ in 0..10_000 {
            if !crate::process::list_local_pids()
                .into_iter()
                .any(|p| spawned.contains(&p))
            {
                break;
            }
            crate::process::yield_now();
        }
    }
    heap.restore_globals(saved);
    result
}

fn try_catch(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let handler = arg(args, 1);
    // The thunk runs through `apply`, which can collect at ANY eval depth
    // (ADR-061). On the error path we still need `handler` and `env` afterwards,
    // so root them on the operand stack across the thunk and re-read the
    // relocated handles. (The thrown value / built error map is fresh after the
    // unwind — no safepoint runs while an `Err` propagates — so it needs no
    // rooting.) This is the `(try (loop) (catch e …))` supervised-server shape.
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_root(handler);
    heap.push_env_root(env);
    let outcome = apply_engine(heap, thunk, &[], env);
    let handler = heap.root_at(vb);
    let env = heap.env_root_at(eb);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    match outcome {
        Ok(value) => Ok(value),
        // A control signal (a `receive` suspend, ADR-100 §7) is **not** an error —
        // re-raise it untouched so it reaches the bytecode driver / scheduler. `%try`
        // must never catch it: it isn't a `throw`/error, and unwinding to the handler
        // here would discard the captured continuation the suspend means to resume.
        Err(e) if e.is_control() => Err(e),
        Err(e) => {
            // The catch sees:
            //   * the user-thrown value verbatim, if there is one (preserves the
            //     "throw shape == catch shape" contract — `(throw 42)` → 42);
            //   * **a structured map** for any built-in error, so Brood code (and
            //     agents via MCP) can `(case (get e :kind) :unbound …)` without
            //     parsing strings (`docs/llm-native.md` §4). Shape on
            //     `LispError::to_value_map`: `{:kind :message [:code] [:file
            //     :line :col] [:hint]}`.
            let caught = match e.payload {
                Some(v) => v,
                None => e.to_value_map(heap),
            };
            apply_engine(heap, handler, &[caught], env)
        }
    }
}

// ----- dynamic variables -----------------------------------------------------
//
// The kernel for `defdyn`/`binding`; the surface macros are in the prelude. A
// dynamic variable's *value* resolves through the per-process binding stack in
// the `Heap` (see `Heap::env_get`), so reads need no primitive here — only the
// declaration, the scoped rebind, and the predicate.

/// `(%declare-dynamic 'name)` — mark a symbol as a dynamic variable, so
/// `binding` will accept it (and `dynamic?` reports it). `defdyn` expands to
/// this plus a plain `def` of the default value.
fn declare_dynamic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%declare-dynamic", arg(args, 0))?;
    value::mark_dynamic(sym);
    Ok(Value::Sym(sym))
}

/// `(%in-ns 'foo)` — set the namespace being compiled into (ADR-065). Emitted by
/// the `ns` macro; the resolver pass qualifies subsequent definitions and free
/// references to `foo/…`. Returns the namespace symbol.
fn in_ns(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%in-ns", arg(args, 0))?;
    heap.set_compile_ns(Some(sym));
    Ok(Value::Sym(sym))
}

/// `(current-ns)` — the namespace currently being compiled into (a symbol), or
/// `nil` at root. Reflection + a handle for tests (ADR-065).
fn current_ns(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.compile_ns().map(Value::Sym).unwrap_or(Value::Nil))
}

/// `(%refer 'mod subset)` — add `(:use …)` imports to the current file's import
/// table (ADR-065 inc-2). `mod` must already be loaded (the `ns` macro emits a
/// `(require 'mod)` first). `subset` nil → refer every *public* `mod/name` (no
/// `--` private marker, not itself nested); else a seq of bare symbols → refer
/// just those as `mod/name`. Each becomes a bare → qualified entry the resolver
/// consults after the current namespace and before root.
fn refer(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mod_sym = expect_symbol(heap, "%refer", arg(args, 0))?;
    let mod_name = value::symbol_name(mod_sym);
    let prefix = format!("{}/", mod_name);
    match arg(args, 1) {
        Value::Nil => {
            // Refer all public names: enumerate the live globals under `mod/`.
            for g in heap.global_symbols() {
                let name = value::symbol_name(g);
                if let Some(bare) = name.strip_prefix(&prefix) {
                    if !bare.is_empty() && !bare.contains('/') && !bare.contains("--") {
                        let bare_sym = value::intern(bare);
                        heap.add_import(bare_sym, g);
                    }
                }
            }
        }
        subset => {
            // Refer just the named symbols as `mod/name` (existence not required —
            // an unbound `mod/name` surfaces as a normal unbound-reference error).
            for item in heap.seq_items(subset)? {
                let bare = expect_symbol(heap, "%refer", item)?;
                let qualified =
                    value::intern(&format!("{}/{}", mod_name, value::symbol_name(bare)));
                heap.add_import(bare, qualified);
            }
        }
    }
    Ok(Value::Nil)
}

/// `(%alias module short)` — register a module alias (Elixir-style): a later
/// qualified reference `short/name` resolves to `module/name`. Stored in the import
/// table under the slash-suffixed key `short/`, so it rides the same per-file
/// lifecycle as `%refer`. The `(:alias …)` header emits it. A second `short` for a
/// different module is a loud error (the ambiguous-last-segment case — disambiguate
/// with an explicit `:as`).
fn alias(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let module = expect_symbol(heap, "%alias", arg(args, 0))?;
    let short = expect_symbol(heap, "%alias", arg(args, 1))?;
    let key = value::intern(&format!("{}/", value::symbol_name(short)));
    if let Some(prev) = heap.import_of(key) {
        if prev != module {
            return Err(LispError::runtime(format!(
                "alias `{}` is already bound to `{}` — can't also alias `{}`; give one an explicit `:as` name",
                value::symbol_name(short),
                value::symbol_name(prev),
                value::symbol_name(module),
            )));
        }
    }
    heap.add_import(key, module);
    Ok(Value::Nil)
}

/// `(dynamic? x)` — true when `x` is a symbol declared dynamic with `defdyn`.
/// A non-symbol is simply not dynamic (no error), so it composes in predicates.
fn dynamic_p(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(
        matches!(arg(args, 0), Value::Sym(s) if value::is_dynamic(s)),
    ))
}

/// `(%binding syms vals thunk)` — run `thunk` (no args) with each dynamic var in
/// `syms` bound to the matching value in `vals` for the dynamic extent of the
/// call, restoring the previous bindings on return *or* error. `syms` (a quoted
/// list) and `vals` (a vector) are equal-length sequences built by the `binding`
/// macro — both emitted as unshadowable literals, so a local rebinding of `list`
/// can't break the form. Every name must be declared dynamic (else it's almost
/// certainly a typo — a plain global won't track the rebind). The bindings live
/// in this process's heap, so they don't reach a `spawn`ed child.
fn binding(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let syms = heap.seq_items(arg(args, 0))?;
    let vals = heap.seq_items(arg(args, 1))?;
    let thunk = arg(args, 2);
    // Validate every name up front, before pushing anything — so a bad `binding`
    // leaves the dynamic stack untouched rather than half-pushed.
    let mut names = Vec::with_capacity(syms.len());
    for s in &syms {
        let sym = expect_symbol(heap, "binding", *s)?;
        if !value::is_dynamic(sym) {
            return Err(LispError::runtime(format!(
                "binding: {} is not a dynamic variable (declare it with defdyn)",
                value::symbol_name(sym)
            )));
        }
        names.push(sym);
    }
    for (i, &sym) in names.iter().enumerate() {
        heap.push_dynamic(sym, arg(&vals, i));
    }
    let result = apply_engine(heap, thunk, &[], env);
    for _ in 0..names.len() {
        heap.pop_dynamic();
    }
    result
}

#[cfg(test)]
mod gui_face_tests {
    use super::gui_face;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    // `gui_face` is the seam between a Brood face map and the GUI backend; verify it
    // reads the per-section font keys (`:family`/`:italic`) + flags. No window needed.
    #[test]
    fn reads_family_italic_and_flags() {
        let mut heap = Heap::new();
        let mono = value::intern("mono");
        let face = heap.map_from_pairs(vec![
            (value::kw("fg"), value::kw("red")),
            (value::kw("bold"), Value::Bool(true)),
            (value::kw("italic"), Value::Bool(true)),
            (value::kw("underline"), Value::Bool(true)),
            (value::kw("family"), Value::Keyword(mono)),
        ]);
        let f = gui_face(&heap, face);
        assert_eq!(f.fg, Some([0xcd, 0x31, 0x31]));
        assert!(f.bold);
        assert!(f.italic);
        assert!(f.underline);
        assert_eq!(f.family, Some(mono));
    }

    // A non-map (or nil) face is the default face: no colours, no flags, no family.
    #[test]
    fn non_map_face_is_default() {
        let heap = Heap::new();
        let f = gui_face(&heap, Value::Nil);
        assert!(f.fg.is_none());
        assert!(!f.bold && !f.italic && !f.underline && !f.reverse);
        assert!(f.family.is_none());
    }

    // A curated palette needs explicit colours, not just the 16 named slots: an
    // `[r g b]` vector and a `"#rrggbb"` hex string both resolve to that true colour.
    #[test]
    fn fg_accepts_rgb_vector_and_hex_string() {
        let mut heap = Heap::new();
        let triple = heap.alloc_vector(vec![Value::Int(0x28), Value::Int(0x2c), Value::Int(0x34)]);
        let by_vec = heap.map_from_pairs(vec![(value::kw("fg"), triple)]);
        assert_eq!(gui_face(&heap, by_vec).fg, Some([0x28, 0x2c, 0x34]));

        let hex = heap.alloc_string("#61afef");
        let by_hex = heap.map_from_pairs(vec![(value::kw("bg"), hex)]);
        assert_eq!(gui_face(&heap, by_hex).bg, Some([0x61, 0xaf, 0xef]));
    }
}

#[cfg(test)]
mod color_value_tests {
    use super::{face_rgb, parse_hex_color};
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    #[test]
    fn parses_six_and_three_digit_hex() {
        assert_eq!(parse_hex_color("#61afef"), Some([0x61, 0xaf, 0xef]));
        assert_eq!(parse_hex_color("#f0a"), Some([0xff, 0x00, 0xaa])); // nibble doubling
        assert_eq!(parse_hex_color("#000000"), Some([0, 0, 0]));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(parse_hex_color("61afef"), None); // no leading #
        assert_eq!(parse_hex_color("#12g456"), None); // non-hex digit
        assert_eq!(parse_hex_color("#1234"), None); // bad length
        assert_eq!(parse_hex_color("#"), None);
    }

    #[test]
    fn face_rgb_spans_keyword_vector_and_hex() {
        let mut heap = Heap::new();
        // a palette keyword still resolves via the shared path
        assert_eq!(face_rgb(&heap, value::kw("red")), Some([0xcd, 0x31, 0x31]));
        // an explicit vector, clamped to 0..255
        let v = heap.alloc_vector(vec![Value::Int(300), Value::Int(-5), Value::Int(128)]);
        assert_eq!(face_rgb(&heap, v), Some([255, 0, 128]));
        // a hex string
        let s = heap.alloc_string("#282c34");
        assert_eq!(face_rgb(&heap, s), Some([0x28, 0x2c, 0x34]));
        // anything else is the default face
        assert_eq!(face_rgb(&heap, Value::Int(7)), None);
    }
}

#[cfg(test)]
mod cursor_style_tests {
    use super::cursor_style_from;
    use crate::core::value::{self, Value};
    use crate::gui::CursorStyle;

    #[test]
    fn maps_keywords_with_block_default() {
        assert_eq!(cursor_style_from(value::kw("bar")), CursorStyle::Bar);
        assert_eq!(
            cursor_style_from(value::kw("underline")),
            CursorStyle::Underline
        );
        assert_eq!(cursor_style_from(value::kw("block")), CursorStyle::Block);
        // a bare `[:cursor row col]` (no style) and any unknown keyword → Block
        assert_eq!(cursor_style_from(Value::Nil), CursorStyle::Block);
        assert_eq!(cursor_style_from(value::kw("wat")), CursorStyle::Block);
    }
}

#[cfg(test)]
mod mouse_event_tests {
    use super::mouse_to_value;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};
    use crossterm::event::{KeyModifiers, MouseButton as CB, MouseEvent, MouseEventKind as MK};

    fn ev(kind: MK) -> MouseEvent {
        MouseEvent {
            kind,
            column: 7,
            row: 3,
            modifiers: KeyModifiers::empty(),
        }
    }

    // The `[:mouse action button row col]` shape: pull out the action keyword (idx 1)
    // and the button keyword (idx 2) as interned ids — `Value` has no `PartialEq`, so
    // we compare the underlying `u32`s. Lets us assert the crossterm → Brood mapping,
    // including the newly added :drag / :release (ADR-077).
    fn action_button(heap: &Heap, v: Value) -> (u32, u32) {
        let Value::Vector(id) = v else {
            panic!("expected a [:mouse …] vector, got {v:?}");
        };
        let xs = heap.vector(id);
        let (Value::Keyword(head), Value::Keyword(a), Value::Keyword(b)) = (xs[0], xs[1], xs[2])
        else {
            panic!("expected keywords for head/action/button, got {xs:?}");
        };
        assert_eq!(head, value::intern("mouse"));
        (a, b)
    }

    #[test]
    fn drag_and_release_map_to_keywords_carrying_their_button() {
        let mut heap = Heap::new();

        let v = mouse_to_value(&mut heap, ev(MK::Drag(CB::Left)));
        let (a, b) = action_button(&heap, v);
        assert_eq!(a, value::intern("drag"));
        assert_eq!(b, value::intern("left"));

        let v = mouse_to_value(&mut heap, ev(MK::Up(CB::Right)));
        let (a, b) = action_button(&heap, v);
        assert_eq!(a, value::intern("release"));
        assert_eq!(b, value::intern("right"));

        let v = mouse_to_value(&mut heap, ev(MK::Down(CB::Middle)));
        let (a, b) = action_button(&heap, v);
        assert_eq!(a, value::intern("press"));
        assert_eq!(b, value::intern("middle"));
    }

    // Bare motion (no button held) still isn't surfaced — it stays nil, as before, so
    // the input channel isn't flooded with per-cell moves when nothing is dragging.
    #[test]
    fn bare_motion_is_not_emitted() {
        let mut heap = Heap::new();
        assert!(matches!(
            mouse_to_value(&mut heap, ev(MK::Moved)),
            Value::Nil
        ));
    }

    // Held modifiers ride on the event as a trailing `[:ctrl …]` vector (so an app
    // can bind Ctrl+wheel for zoom). No modifiers → an empty vector, not absent.
    #[test]
    fn modifiers_ride_on_the_event() {
        let mut heap = Heap::new();

        // Ctrl held during a scroll → mods is `[:ctrl]` at index 5.
        let ctrl_scroll = MouseEvent {
            kind: MK::ScrollUp,
            column: 7,
            row: 3,
            modifiers: KeyModifiers::CONTROL,
        };
        let Value::Vector(id) = mouse_to_value(&mut heap, ctrl_scroll) else {
            panic!("expected a [:mouse …] vector");
        };
        let xs = heap.vector(id).to_vec();
        assert_eq!(xs.len(), 6, "event now carries a trailing mods vector");
        let Value::Vector(mid) = xs[5] else {
            panic!("mods should be a vector, got {:?}", xs[5]);
        };
        let mods = heap.vector(mid);
        assert_eq!(mods.len(), 1);
        assert!(matches!(mods[0], Value::Keyword(k) if k == value::intern("ctrl")));

        // No modifiers → an empty mods vector (present, so destructuring is stable).
        let Value::Vector(id) = mouse_to_value(&mut heap, ev(MK::ScrollUp)) else {
            panic!("expected a [:mouse …] vector");
        };
        let xs = heap.vector(id).to_vec();
        let Value::Vector(mid) = xs[5] else {
            panic!("mods should be a vector");
        };
        assert!(heap.vector(mid).is_empty());
    }

    // A press carries a trailing click-chain count (the 7th element); the terminal
    // can't detect multi-click, so it always reports 1. Non-press actions omit it.
    #[test]
    fn a_press_carries_a_trailing_count_others_do_not() {
        let mut heap = Heap::new();

        let Value::Vector(id) = mouse_to_value(&mut heap, ev(MK::Down(CB::Left))) else {
            panic!("expected a [:mouse …] vector");
        };
        let xs = heap.vector(id).to_vec();
        assert_eq!(xs.len(), 7, "a press has the trailing count");
        assert!(matches!(xs[6], Value::Int(1)), "terminal press counts as 1");

        // A release stays 6-element (no count).
        let Value::Vector(id) = mouse_to_value(&mut heap, ev(MK::Up(CB::Left))) else {
            panic!("expected a [:mouse …] vector");
        };
        assert_eq!(heap.vector(id).len(), 6, "a release has no trailing count");
    }
}
