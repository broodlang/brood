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
use crate::eval::apply;
use crate::types::{Sig, Ty};

mod numeric;
mod sequences;
mod io;
mod terminal;
mod system;

use numeric::*;
use sequences::*;
use io::*;
use terminal::*;
use system::*;

pub use io::{begin_stdout_capture, take_captured_stdout};
pub use system::SPECIAL_FORMS;
pub use terminal::{restore_terminal, restore_raw, restore_terminal_on_exit};

pub fn realize_seqview(heap: &mut Heap, env: EnvId, sv: Value) -> LispResult {
    let f = heap
        .env_get(heap.global(), value::intern("%seqview-realize"))
        .ok_or_else(|| LispError::runtime("%seqview-realize is not defined".to_string()))?;
    apply(heap, f, &[sv], env)
}

#[allow(non_upper_case_globals)]
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
    const bitset_ty: Ty = Ty::of(Tag::Bitset);
    const kw: Ty = Ty::of(Tag::Keyword);
    const sym: Ty = Ty::of(Tag::Sym);
    const bool_ty: Ty = Ty::of(Tag::Bool);
    const nil_ty: Ty = Ty::of(Tag::Nil);
    const pair: Ty = Ty::of(Tag::Pair);
    const vec_ty: Ty = Ty::of(Tag::Vector);
    const map_ty: Ty = Ty::of(Tag::Map);
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
        "max",
        Arity::at_least(1),
        Sig::variadic(num, num),
        prim_max,
    );
    def(
        heap,
        "min",
        Arity::at_least(1),
        Sig::variadic(num, num),
        prim_min,
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
    // A `bitset` is its own `Value::Bitset` kind (KI-4) — raw bit-data bytes (LSB-first, bit i
    // = byte i/8 bit i%8) in an `Arc<SharedBlob>`, NEVER a UTF-8 `Str` (that corrupted the GC
    // on promote). ALWAYS stored shared, so it crosses `send`/`table` by reference (Arc bump),
    // not a copy — unlike a bignum, which serialises to decimal. Ops are O(bytes) native loops.
    def(heap, "bitset", Arity::exact(1), Sig::new(vec![int], bitset_ty), bs_make);
    def(heap, "bitset-ones", Arity::exact(1), Sig::new(vec![int], bitset_ty), bs_ones);
    def(heap, "bitset-and", Arity::exact(2), Sig::new(vec![bitset_ty, bitset_ty], bitset_ty), bs_and);
    def(heap, "bitset-or", Arity::exact(2), Sig::new(vec![bitset_ty, bitset_ty], bitset_ty), bs_or);
    def(heap, "bitset-xor", Arity::exact(2), Sig::new(vec![bitset_ty, bitset_ty], bitset_ty), bs_xor);
    def(heap, "bitset-shl", Arity::exact(2), Sig::new(vec![bitset_ty, int], bitset_ty), bs_shl);
    def(heap, "bitset-shr", Arity::exact(2), Sig::new(vec![bitset_ty, int], bitset_ty), bs_shr);
    def(heap, "bitset-set", Arity::exact(2), Sig::new(vec![bitset_ty, int], bitset_ty), bs_set);
    def(heap, "bitset-count", Arity::exact(1), Sig::new(vec![bitset_ty], int), bs_count);
    def(heap, "bitset-positions", Arity::exact(1), Sig::new(vec![bitset_ty], vec_ty), bs_positions);
    def(heap, "bitset-planes", Arity::exact(1), Sig::new(vec![vec_ty], vec_ty), bs_planes);
    def(
        heap,
        "bitset-neighbour-sum",
        Arity::exact(7),
        Sig::new(vec![bitset_ty, bitset_ty, bitset_ty, bitset_ty, bitset_ty, int, int], vec_ty),
        bs_neighbour_sum,
    );
    def(
        heap,
        "bitset-life-step",
        Arity::exact(7),
        Sig::new(vec![bitset_ty, bitset_ty, bitset_ty, bitset_ty, bitset_ty, int, int], bitset_ty),
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
    def(
        heap,
        "nil?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        is_nil,
    );
    def(
        heap,
        "pair?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        is_pair,
    );
    def(
        heap,
        "empty?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        is_empty,
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
    // Lazy seq-view (ADR: lazy seq-view) — the fused result of `map`/`filter`/
    // `keep`/`remove`. `%seqview` constructs it from `[source xform]`;
    // `%seqview-parts` returns that pair as a 2-vector for the prelude `fold`
    // fusion / realisation; `seqview?` is the fold-family fast-path predicate.
    // A view carries `tag = Pair` (it is the list it stands in for), hence `pair`.
    def(
        heap,
        "%seqview",
        Arity::exact(2),
        Sig::new(vec![any, callable], pair),
        seqview_make,
    );
    def(
        heap,
        "%seqview-parts",
        Arity::exact(1),
        Sig::new(vec![any], vec_ty),
        seqview_parts,
    );
    def(
        heap,
        "seqview?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        seqview_pred,
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
        "map-int-add",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, int], map_ty),
        map_int_add,
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
    def(
        heap,
        "%f64-sqrt",
        Arity::exact(1),
        Sig::new(vec![num], float),
        math_f64_sqrt,
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
        "tls-listen",
        Arity::exact(4),
        Sig::new(vec![string, int, string, string], socket_ty),
        tls_listen,
    );
    def(
        heap,
        "tls-self-signed",
        Arity::exact(1),
        Sig::new(vec![string], list_ty),
        tls_self_signed,
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
        "proc-set-binary",
        Arity::exact(2),
        Sig::new(vec![subprocess_ty, any], nil_ty),
        proc_set_binary,
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
        "audio-beep",
        Arity::range(2, 3),
        Sig::with_rest(vec![num, num], num, nil_ty),
        audio_beep,
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
        "slurp-bytes",
        Arity::exact(1),
        Sig::new(vec![string], seq),
        slurp_bytes,
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
    // Raw-byte digests: byte vector → digest as a byte vector (not hex). Lets a
    // digest chain over raw bytes (SCRAM's StoredKey = SHA256(ClientKey), then
    // HMAC over StoredKey) without a hex decode at each step (store findings #3).
    def(
        heap,
        "%sha256-raw",
        Arity::exact(1),
        Sig::new(vec![any], seq),
        sha256_raw,
    );
    def(
        heap,
        "%sha1-raw",
        Arity::exact(1),
        Sig::new(vec![any], seq),
        sha1_raw,
    );
    def(
        heap,
        "%sha384-raw",
        Arity::exact(1),
        Sig::new(vec![any], seq),
        sha384_raw,
    );
    def(
        heap,
        "%sha512-raw",
        Arity::exact(1),
        Sig::new(vec![any], seq),
        sha512_raw,
    );
    def(
        heap,
        "%md5-raw",
        Arity::exact(1),
        Sig::new(vec![any], seq),
        md5_raw,
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
    // Raw-byte HMAC: byte-vector key + message → byte-vector MAC. A string can't
    // carry an arbitrary-byte key faithfully, and SCRAM XORs/re-hashes the raw
    // MAC bytes, so the string-keyed %hmac-* can't serve binary auth (findings #2).
    def(
        heap,
        "%hmac-sha256-raw",
        Arity::exact(2),
        Sig::new(vec![any, any], seq),
        hmac_sha256_raw,
    );
    def(
        heap,
        "%hmac-sha1-raw",
        Arity::exact(2),
        Sig::new(vec![any, any], seq),
        hmac_sha1_raw,
    );
    def(
        heap,
        "%hmac-sha512-raw",
        Arity::exact(2),
        Sig::new(vec![any, any], seq),
        hmac_sha512_raw,
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
        "%spawn-link",
        Arity::exact(1),
        Sig::new(vec![callable], pid_ty),
        spawn_link,
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
        "%pbkdf2-sha256-bytes",
        Arity::exact(4),
        Sig::new(vec![any, any, int, int], seq),
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
    ("nil?", &["x"], "True if x is nil."),
    ("pair?", &["x"], "True if x is a cons pair."),
    ("empty?", &["coll"], "True if coll is empty (nil, an empty string/vector/map, or a seq-view that realises to nothing)."),
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
    ("map-int-add", &["m", "k", "delta"], "A fresh map like m with key k's integer value incremented by delta (inserts delta when k is absent). Single trie traversal — equivalent to (assoc m k (+ (get m k 0) delta)) without the extra walk."),
    ("map-dissoc", &["m", "k"], "A fresh map like m with key k removed."),
    ("map-pairs", &["m"], "The entries of m as a list of [k v] vectors, in insertion order."),
    ("map-count", &["m"], "The number of entries in map m. O(1) — the CHAMP root tracks its size."),
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
    ("%f64-sqrt", &["x"], "The IEEE 754 square root of x (f64::sqrt). x must be non-negative; raises otherwise. Handles subnormals and ±0 correctly."),
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
    ("tls-listen", &["host", "port", "cert-pem", "key-pem"], "Bind a TLS listening socket on host:port using the PEM certificate chain cert-pem and private key key-pem (port 0 = OS-assigned). Like tcp-listen, connections arrive as [:tcp-accept lsock client]; each accepted socket transparently decrypts inbound to [:tcp …] and encrypts tcp-send, so code above the transport is unchanged. Returns a socket."),
    ("tls-self-signed", &["host"], "Generate a self-signed TLS certificate + private key for host (a DNS name like \"localhost\"), for zero-config dev TLS. Returns [cert-pem key-pem] — pass them to tls-listen. Not for production (clients reject a self-signed cert unless told to trust it)."),
    ("tcp-send", &["sock", "s"], "Write the whole string s to sock (blocking). Text mode (default): s is sent as UTF-8. Binary mode (see tcp-set-binary): each codepoint of s must be 0–255 and is written as one raw byte. Returns nil; throws on error."),
    ("tcp-set-binary", &["sock", "on"], "Switch sock between text mode (default) and binary mode. In binary mode inbound [:tcp …] data is a byte-faithful Latin-1 string (one codepoint 0–255 per byte received) and tcp-send writes each codepoint 0–255 as one raw byte — for length-prefixed / control-byte protocols like WebSocket framing. Returns nil; throws if sock is gone or a listener."),
    ("tcp-controlling-process", &["sock", "pid"], "Make pid the owner of sock's inbound data: starts reading a just-accepted (passive) socket, or retargets an active one. Returns nil."),
    ("tcp-close", &["sock"], "Close sock (a stream or listener), releasing its fd / stopping its accept loop. Idempotent; returns nil."),
    ("tcp-local-port", &["sock"], "The local port sock is bound to, or nil."),
    ("proc-spawn", &["prog", "args", "opts"], "Spawn prog (a string) with args (a list/vector of strings) as a persistent child process with piped stdio. An optional opts map tunes the child: :cwd (a string) sets its working directory, :env (a map of string->string) adds environment variables on top of the inherited environment. Its stdout/stderr arrive at the calling process as [:proc handle data] / [:proc-err handle data] messages, and [:proc-closed handle code] on exit (code is the exit status, or nil if signalled). Returns a subprocess handle. Throws if prog can't be spawned."),
    ("proc-send", &["p", "s"], "Write the whole string s to subprocess p's stdin (blocking) and flush. Returns nil; throws if p is unknown/closed. In binary mode (see proc-set-binary) s must be a byte-string (codepoints 0–255), written as raw bytes."),
    ("proc-set-binary", &["p", "on"], "Switch subprocess p between text mode (default) and binary mode (mirrors tcp-set-binary). In binary mode inbound [:proc …]/[:proc-err …] data is a byte-faithful Latin-1 string (one codepoint 0–255 per byte) and proc-send writes each codepoint as one raw byte — for a child speaking a binary protocol over stdio. Returns nil; throws if p is unknown/closed."),
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
    ("slurp", &["path"], "Read the whole file at path into a string (does not evaluate it). UTF-8; throws on a non-text file — use slurp-bytes for binary."),
    ("slurp-bytes", &["path"], "Read the whole file at path as a byte vector (ints 0–255). The byte-faithful read slurp can't be (slurp is UTF-8 and throws on a non-text file). Pairs with %sha256-bytes/%sha256-raw and the encoding byte variants — e.g. hashing a binary asset."),
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
    ("%sha256-raw",   &["bytes"], "SHA-256 of a byte vector, returned as a 32-byte vector (raw digest, not hex). For chaining digests over raw bytes without a hex round-trip at each step."),
    ("%sha1-raw",     &["bytes"], "SHA-1 of a byte vector, returned as a 20-byte vector (raw digest, not hex)."),
    ("%sha384-raw",   &["bytes"], "SHA-384 of a byte vector, returned as a 48-byte vector (raw digest, not hex)."),
    ("%sha512-raw",   &["bytes"], "SHA-512 of a byte vector, returned as a 64-byte vector (raw digest, not hex)."),
    ("%md5-raw",      &["bytes"], "MD5 of a byte vector, returned as a 16-byte vector (raw digest, not hex)."),
    ("%hmac-sha256", &["key", "message"], "HMAC-SHA256 of `message` keyed with `key` (both strings). Returns lowercase hex. RFC 2104 over sha2."),
    ("%hmac-sha1",   &["key", "message"], "HMAC-SHA1 of `message` keyed with `key`. Returns lowercase hex. Not collision-resistant; prefer hmac-sha256."),
    ("%hmac-sha512", &["key", "message"], "HMAC-SHA512 of `message` keyed with `key`. Returns lowercase hex."),
    ("%hmac-sha256-raw", &["key-bytes", "msg-bytes"], "HMAC-SHA256 over a byte-vector key and message, returned as a 32-byte vector. For binary-protocol auth (SCRAM) where the key is raw bytes and the MAC is XORed/re-hashed."),
    ("%hmac-sha1-raw",   &["key-bytes", "msg-bytes"], "HMAC-SHA1 over a byte-vector key and message, returned as a 20-byte vector."),
    ("%hmac-sha512-raw", &["key-bytes", "msg-bytes"], "HMAC-SHA512 over a byte-vector key and message, returned as a 64-byte vector."),
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
    ("%pbkdf2-sha256-bytes", &["password-bytes", "salt-bytes", "iterations", "key-len"], "PBKDF2-HMAC-SHA256 key derivation over byte-vector password and salt (raw bytes, not UTF-8 strings — a binary salt round-trips faithfully). Returns a key-len-byte vector. Use iterations >= 600000 for password storage."),
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
    ("%spawn-link", &["thunk"], "Like %spawn but atomically links the child to the caller before it runs (no spawn->link :noproc race). Use the `spawn-link` macro."),
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
    ("audio-beep", &["freq-hz", "ms", "vol"], "Play a short tone of freq-hz for ms milliseconds, optionally at peak amplitude vol (0..1, default ~0.18 — pass a small vol for quiet/ambient sounds). Fire-and-forget — it never blocks the caller, and overlapping beeps mix — so a game can blip from its frame loop. Synthesised on a dedicated audio thread (needs --features audio). A graceful no-op without the feature, when there's no audio device, or when muted via BROOD_AUDIO=0 or BROOD_GUI_HEADLESS. Returns nil."),
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

