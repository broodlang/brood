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

mod bytes;
mod crypto;
mod errors;
mod io;
mod numeric;
mod os;
mod pkg;
mod selfhost_macros;
mod sequences;
mod syntax_scan;
mod system;
mod terminal;
mod tooling;

// The boot cache (`lib.rs`) keys its expanded-prelude file on the build id.
pub(crate) use system::build_id_string;

use bytes::*;
use crypto::*;
use errors::*;
use io::*;
use numeric::*;
use os::*;
use pkg::*;
use selfhost_macros::*;
use sequences::*;
use syntax_scan::*;
use system::*;
use terminal::*;
use tooling::*;

pub use io::{arm_mcp_progress, begin_stdout_capture, disarm_mcp_progress, take_captured_stdout};
pub use terminal::{restore_raw, restore_terminal, restore_terminal_on_exit};
pub use tooling::SPECIAL_FORMS;

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
    const bytes_ty: Ty = Ty::of(Tag::Bytes);
    const decimal_ty: Ty = Ty::of(Tag::Decimal);
    const ratio_ty: Ty = Ty::of(Tag::Ratio);
    const kw: Ty = Ty::of(Tag::Keyword);
    const sym: Ty = Ty::of(Tag::Sym);
    const bool_ty: Ty = Ty::of(Tag::Bool);
    const nil_ty: Ty = Ty::of(Tag::Nil);
    const pair: Ty = Ty::of(Tag::Pair);
    const vec_ty: Ty = Ty::of(Tag::Vector);
    const map_ty: Ty = Ty::of(Tag::Map);
    const set_ty: Ty = Ty::of(Tag::Set);
    const pid_ty: Ty = Ty::of(Tag::Pid);
    const ref_ty: Ty = Ty::of(Tag::Ref);
    const list_ty: Ty = Ty::LIST;
    // `bytes` is seqable too: `first`/`rest`/`nth` iterate its octets at runtime.
    const seq: Ty = Ty::of_tags(&[Tag::Nil, Tag::Pair, Tag::Vector, Tag::Bytes]);
    // `first`/`rest` additionally walk a **set** (as its elements) and a **map** (as
    // its `[k v]` pairs), matching `seq`/`map`/`fold`/`last`. Kept separate from
    // `seq` so widening the head/tail pair doesn't silently widen every other
    // sequence primitive's domain.
    const seqable: Ty = Ty::of_tags(&[
        Tag::Nil,
        Tag::Pair,
        Tag::Vector,
        Tag::Bytes,
        Tag::Set,
        Tag::Map,
    ]);
    const callable: Ty = Ty::of_tags(&[Tag::Fn, Tag::Native]);
    // An **iolist** (ADR-139): a string, a `bytes`, a byte int 0–255, or an
    // arbitrarily nested list/vector of iolists (nil = empty). The lattice can't
    // express the recursion, so this is the shallow surface — the runtime
    // flattener (`flatten_iolist`) enforces the leaves.
    const iolist: Ty = Ty::of_tags(&[
        Tag::Str,
        Tag::Bytes,
        Tag::Int,
        Tag::Pair,
        Tag::Vector,
        Tag::Nil,
    ]);

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
    // `min`/`max` accept a number OR an `Ord` record — they route through the `compare-to`
    // multimethod on the record cold path (ADR-179), returning the same domain.
    let num_or_record = num.union(map_ty);
    def(
        heap,
        "max",
        Arity::at_least(1),
        Sig::variadic(num_or_record.clone(), num_or_record.clone()),
        prim_max,
    );
    def(
        heap,
        "min",
        Arity::at_least(1),
        Sig::variadic(num_or_record.clone(), num_or_record),
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
    // Ratio parts + conversions (exact rationals, ADR-196). `numerator`/`denominator`
    // accept an int (numerator = itself, denominator = 1) or a ratio.
    let int_or_ratio = int.union(ratio_ty);
    def(
        heap,
        "numerator",
        Arity::exact(1),
        Sig::new(vec![int_or_ratio.clone()], int),
        prim_numerator,
    );
    def(
        heap,
        "denominator",
        Arity::exact(1),
        Sig::new(vec![int_or_ratio], int),
        prim_denominator,
    );
    def(
        heap,
        "->decimal",
        Arity::exact(1),
        Sig::new(vec![num], decimal_ty),
        prim_to_decimal,
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
    // Bit-level reinterpretation of a binary64 — not expressible over the other
    // primitives (no bitcast, no frexp), and the only way to compare two floats
    // *exactly* (`-0.0` vs `0.0`, NaN payloads). Used by the conformance corpora.
    def(
        heap,
        "float->bits",
        Arity::exact(1),
        Sig::new(vec![num], int),
        float_to_bits,
    );
    def(
        heap,
        "bits->float",
        Arity::exact(1),
        Sig::new(vec![int], float),
        bits_to_float,
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
        Sig::new(vec![seqable], any),
        first,
    );
    def(
        heap,
        "rest",
        Arity::exact(1),
        Sig::new(vec![seqable], list_ty),
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
    // Ability dispatch through the per-op inline cache (ADR-172 §7): (impls, op-key, id) →
    // impl fn or nil. Internal; the op `defability` emits calls it, never user code.
    def(
        heap,
        "%dispatch",
        Arity::exact(3),
        Sig::new(vec![map_ty, any, any], any),
        dispatch,
    );

    // Atomic registry update (KI-22): the read-modify-write of a global holding a whole
    // registry map, done in ONE kernel call so two concurrent registrations cannot each
    // read the old map and clobber each other. Internal; `register-impl`/`provide`/
    // `defability`/… in the prelude call it, never user code.
    def(
        heap,
        "%registry-update!",
        Arity::exact(4),
        Sig::new(vec![any, any, any, any], any),
        registry_update,
    );

    // set (the `#{…}` kernel type; the `set` library is Brood over these)
    def(
        heap,
        "%set",
        Arity::at_least(0),
        Sig::variadic(any, set_ty),
        set_construct,
    );
    def(
        heap,
        "%set-add",
        Arity::exact(2),
        Sig::new(vec![set_ty, any], set_ty),
        set_add,
    );
    def(
        heap,
        "%set-remove",
        Arity::exact(2),
        Sig::new(vec![set_ty, any], set_ty),
        set_remove,
    );
    def(
        heap,
        "%set-has?",
        Arity::exact(2),
        Sig::new(vec![set_ty, any], bool_ty),
        set_has,
    );
    def(
        heap,
        "%set-count",
        Arity::exact(1),
        Sig::new(vec![set_ty], int),
        set_count,
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
    // whole-namespace scan tens of seconds. `index-of` / `includes?`
    // (std/prelude.blsp) ride on this; it's the search counterpart of
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
    // Codepoint access needs Rust for the same reason as split/search: char
    // indexing into UTF-8 is O(index), and the pure-Brood construction
    // (`map char->int` over `string->list`) pays a 1-char string + a closure
    // call per char. One O(n) pass to the vector the text parsers index.
    def(
        heap,
        "string->codepoints",
        Arity::exact(1),
        Sig::new(vec![string], Ty::vector_of(int)),
        string_to_codepoints,
    );
    // Grapheme clusters + normalisation: UAX #29 / UAX #15 table lookups, not rules
    // Brood can express. The cluster is the unit a human calls "a character", so it
    // is what editor cursor motion steps by; normalisation is what makes text that
    // reads the same compare the same under Brood's byte-structural `=`.
    def(
        heap,
        "string->graphemes",
        Arity::exact(1),
        Sig::new(vec![string], Ty::vector_of(string)),
        string_to_graphemes,
    );
    // The indexed grapheme accessors (ADR-159). `string->graphemes` alone made the
    // *documented-correct* cursor step — read the cluster at an index — cost a vector
    // of every cluster in the string, per keystroke. These walk to the index instead.
    def(
        heap,
        "grapheme-count",
        Arity::exact(1),
        Sig::new(vec![string], int),
        grapheme_count,
    );
    def(
        heap,
        "grapheme-at",
        Arity::range(2, 3),
        Sig::new(vec![string, int], any),
        grapheme_at,
    );
    def(
        heap,
        "substring-graphemes",
        Arity::range(2, 3),
        Sig::new(vec![string, int], string),
        substring_graphemes,
    );
    def(
        heap,
        "string-normalize",
        Arity::exact(2),
        Sig::new(vec![string, kw], string),
        string_normalize,
    );
    // The minimal-splice diff of two strings — one O(n) byte pass, char-indexed
    // result. Needs Rust like the search/split above (no O(1) char access), and it
    // is per-keystroke hot: every process-hosted editor buffer diffs old->new text
    // at the loop tail (std/editor/buffer-client `text-splice` rides on this).
    def(
        heap,
        "%str-splice-diff",
        Arity::exact(2),
        Sig::new(vec![string, string], vec_ty),
        str_splice_diff,
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
        Sig::new(vec![string], bytes_ty),
        string_to_utf8_bytes,
    );
    def(
        heap,
        "utf8-bytes->string",
        Arity::exact(1),
        Sig::new(vec![bytes_ty], string),
        utf8_bytes_to_string,
    );
    // ---- raw bytes (Value::Bytes) ----
    def(
        heap,
        "bytes",
        Arity::any(),
        Sig::variadic(any, bytes_ty),
        bytes_make,
    );
    def(
        heap,
        "byte-length",
        Arity::exact(1),
        Sig::new(vec![bytes_ty], int),
        byte_length,
    );
    def(
        heap,
        "byte-at",
        Arity::exact(2),
        Sig::new(vec![bytes_ty, int], int),
        byte_at,
    );
    def(
        heap,
        "subbytes",
        Arity::range(2, 3),
        Sig::variadic(any, bytes_ty),
        subbytes,
    );
    def(
        heap,
        "bytes-concat",
        Arity::any(),
        Sig::variadic(iolist, bytes_ty),
        bytes_concat,
    );
    // String<->bytes conversion is UTF-8 (a Brood string is UTF-8, like Rust's),
    // exposed under the explicit `string->utf8-bytes` / `utf8-bytes->string` names
    // (registered above) — the former duplicate `string->bytes` / `bytes->string`
    // prims were removed (they did the identical UTF-8 encode/decode).
    def(
        heap,
        "bytes->list",
        Arity::exact(1),
        Sig::new(vec![bytes_ty], pair),
        bytes_to_list,
    );
    def(
        heap,
        "bytes-index-of",
        Arity::range(2, 3),
        Sig::new(vec![bytes_ty, bytes_ty], int),
        bytes_index_of,
    );
    // string->number returns int *or* float *or* nil (the parse-failed case).
    def(
        heap,
        "string->number",
        Arity::exact(1),
        Sig::new(vec![string], num.union(nil_ty)),
        string_to_number,
    );
    // `decimal` constructs an exact base-10 decimal from a string ("1.50"), an
    // int (3), or a float (inexact source — uses its shortest round-trip form).
    def(
        heap,
        "decimal",
        Arity::exact(1),
        Sig::new(vec![string.union(num)], decimal_ty),
        numeric::prim_decimal,
    );
    def(
        heap,
        "decimal->string",
        Arity::exact(1),
        Sig::new(vec![decimal_ty], string),
        numeric::prim_decimal_to_string,
    );
    def(
        heap,
        "decimal->float",
        Arity::exact(1),
        Sig::new(vec![decimal_ty], float),
        numeric::prim_decimal_to_float,
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
        Arity::range(3, 4),
        Sig::variadic(any, socket_ty),
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
        Sig::new(vec![socket_ty, iolist], nil_ty),
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
        "tcp-set-idle-timeout",
        Arity::exact(2),
        Sig::new(vec![socket_ty, int], nil_ty),
        tcp_set_idle_timeout,
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
        // data is any iolist (ADR-139); string leaves are UTF-8 in text mode,
        // 0–255 codepoints in binary mode; bytes leaves go verbatim.
        Sig::new(vec![subprocess_ty, iolist], nil_ty),
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
    def(
        heap,
        "table",
        Arity::exact(0),
        Sig::nullary(table_ty),
        table_new,
    );
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
    // timeout; `term-draw` interprets a frame vector. See std/tool/observer.blsp.
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
    // (std/editor/lineedit.blsp); `term-enter`/`term-draw` stay the full-screen path.
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
    // std/tool/observer.blsp's `gui-display` wraps an id as a display map. See gui.rs.
    def(
        heap,
        "gui-open",
        Arity::range(0, 4),
        // Every optional arg is also nil-able in place (`(gui-open title nil nil
        // opts)` opens at the default size), so the params say so — the runtime
        // treats nil as "use the default" for each.
        Sig::new(
            vec![
                string.union(nil_ty),
                int.union(nil_ty),
                int.union(nil_ty),
                map_ty.union(nil_ty),
            ],
            int,
        ),
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
        "gui-minimize!",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        gui_minimize,
    );
    def(
        heap,
        "gui-drag-move",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        gui_drag_move,
    );
    def(
        heap,
        "gui-drag-resize",
        Arity::exact(2),
        Sig::new(vec![int, kw], nil_ty),
        gui_drag_resize,
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
    // The window background (`gui-bg!`): the clear / inset-margin / snap-remainder fill,
    // so a GUI app's padding matches its theme instead of the hardcoded default.
    def(
        heap,
        "gui-bg!",
        Arity::exact(1),
        Sig::new(
            vec![Ty::of_tags(&[
                Tag::Keyword,
                Tag::Vector,
                Tag::Str,
                Tag::Nil,
            ])],
            nil_ty,
        ),
        gui_bg,
    );
    // The one process-introspection accessor the language can't reach from Brood
    // (the mailbox queue lives behind the scheduler registry). Everything else an
    // observer shows — pid id, liveness — is assembled in Brood (std/tool/observer.blsp).
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
        Arity::range(1, 2),
        Sig::new(vec![string, string], any),
        load_string,
    );
    // The embedded-std-module loader: `%load-string` plus the reserved-name
    // exemption, held across the load and released even on a throw (ADR-166).
    def(
        heap,
        "%load-module-source",
        Arity::range(1, 2),
        Sig::new(vec![string, string], any),
        load_module_source,
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
        "scan-source-extract",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        scan_source_extract,
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
    // Incremental re-parse keyed by a buffer id (same CST shape, less work), and
    // a cache-eviction hook for when a buffer closes. §C.
    def(
        heap,
        "tree-sitter-reparse",
        Arity::exact(3),
        Sig::new(vec![int, string, kw], map_ty),
        tree_sitter_reparse,
    );
    def(
        heap,
        "tree-sitter-forget",
        Arity::exact(1),
        Sig::new(vec![int], int),
        tree_sitter_forget,
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
        "%run-program-file",
        Arity::exact(1),
        Sig::new(vec![string], any),
        run_program_file,
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
        "%builtin-module-file",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        builtin_module_file,
    );
    def(
        heap,
        "%builtin-doc",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        builtin_doc,
    );
    // Line-coverage readout (ADR-148 tier 2). Empty unless BROOD_COVERAGE is set.
    def(
        heap,
        "%coverage-lines",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        coverage_lines,
    );
    def(
        heap,
        "%coverage-instrumented",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        coverage_instrumented,
    );
    def(
        heap,
        "%coverage-branches",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        coverage_branches,
    );
    def(
        heap,
        "%coverage-branch-instrumented",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        coverage_branch_instrumented,
    );
    def(
        heap,
        "%coverage-precompile",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        coverage_precompile,
    );
    def(
        heap,
        "%coverage-reset",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        coverage_reset,
    );
    def(
        heap,
        "builtin-modules",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        builtin_modules,
    );
    // Release-bundle mechanism (ADR-038): an app produced by `nest release`
    // carries its source appended to the binary. These let `std/tool/project.blsp`
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
        "canonicalize",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        path_canonicalize,
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
        Sig::new(vec![string, iolist], nil_ty),
        spit,
    );
    def(
        heap,
        "spit-append",
        Arity::exact(2),
        Sig::new(vec![string, iolist], nil_ty),
        spit_append,
    );
    // Atomic compare-and-swap of a file's whole contents, serialised across
    // processes. The mechanism a safe read-modify-write needs when the "modify" is
    // Brood code (`nest add` editing project.blsp) — see `io::file_swap`.
    def(
        heap,
        "%file-swap",
        Arity::exact(4),
        Sig::new(vec![string, string, string, string], bool_ty),
        file_swap,
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
        Sig::new(vec![string], bytes_ty),
        slurp_bytes,
    );
    def(
        heap,
        "spit-bytes",
        Arity::exact(2),
        Sig::new(vec![string, any], nil_ty),
        spit_bytes,
    );
    def(
        heap,
        "append-bytes",
        Arity::exact(2),
        Sig::new(vec![string, any], nil_ty),
        append_bytes,
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
        "image-thumb",
        Arity::exact(3),
        Sig::new(vec![any, int, int], map_ty.union(nil_ty)),
        image_thumb,
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
    // The two hashing primitives. `%digest` and `%hmac` take an algorithm keyword
    // (:md5/:sha1/:sha256/:sha384/:sha512) + byte-sequence input and return the
    // RAW digest/MAC as a bytes value. Everything else — string input (via
    // `string->utf8-bytes`), hex output (via `bytes->hex`), and the public
    // `sha256`/`hmac-sha256`/… names — is Brood policy in std/hash.blsp. (Collapsed
    // the former 15 `%sha*`/`%md5` + 6 `%hmac-*` prims to these two: ADR-006, the
    // variation was pure formatting Brood can do.) The package manager (ADR-037)
    // hashes files/trees in Brood over these.
    def(
        heap,
        "%digest",
        Arity::exact(2),
        Sig::new(vec![kw, any], bytes_ty),
        digest,
    );
    def(
        heap,
        "%hmac",
        Arity::exact(3),
        Sig::new(vec![kw, any, any], bytes_ty),
        hmac,
    );
    // The package manager's git mechanism (ADR-037): resolve a ref to a commit,
    // and clone+checkout a pinned commit. Thin shell-outs to `git`; the cache
    // layout / lock file / conflict policy are all Brood (std/tool/package.blsp).
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
    // Files not committed-clean under a dir (modified/staged/untracked), for a
    // git-aware `nest format --changed` narrower scope. nil if not a git repo.
    def(
        heap,
        "%git-changed-files",
        Arity::exact(1),
        Sig::new(vec![string], pair.union(nil_ty).union(kw)),
        git_changed_files,
    );
    // Extract a gzip'd tar archive into a dir, stripping N leading path components.
    // The tarball source-delivery mechanism (ADR-037 tarball deps); shells to `tar`.
    def(
        heap,
        "%untar-gz",
        Arity::exact(3),
        Sig::new(vec![string, string, int], kw),
        untar_gz,
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
        "%install-interrupt-handler",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        install_interrupt_handler,
    );
    def(
        heap,
        "%restore-interrupt-handler",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        restore_interrupt_handler,
    );
    def(
        heap,
        "%interrupt-taken?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        interrupt_taken,
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
        "%os-cmd-stdin",
        Arity::at_least(3),
        Sig::new(vec![string, seq, string], map_ty),
        os_cmd_stdin,
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
        Arity::range(1, 2),
        // 2nd arg (optional required-mods) is a list OR vector of module names — `any`
        // so a vector closure doesn't trip the arg-type lint on our own callers.
        Sig::with_rest(vec![string], any, list_ty),
        check_file_builtin,
    );
    def(
        heap,
        "check-file-structured",
        Arity::range(1, 2),
        Sig::with_rest(vec![string], any, list_ty),
        check_file_structured,
    );
    def(
        heap,
        "check-file-deps",
        Arity::range(1, 2),
        Sig::with_rest(vec![string], any, any),
        check_file_deps,
    );
    def(
        heap,
        "%module-direct-requires",
        Arity::exact(1),
        Sig::new(vec![string], any),
        module_direct_requires,
    );
    def(
        heap,
        "check-deps-fp",
        Arity::exact(1),
        Sig::new(vec![any], string),
        check_deps_fp,
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
    def(
        heap,
        "type-signature",
        Arity::exact(1),
        Sig::new(vec![sym.union(string)], string.union(nil_ty)),
        type_signature,
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
    // `(%register-sig 'name 'type)` — record a user-declared `(sig …)` for the
    // checker, keyed by the module-qualified global name (resolved as `def` would).
    // The `sig`/`sig!` macros emit it; the checker's `sig_of` consults the store first.
    def(
        heap,
        "%register-sig",
        Arity::exact(2),
        Sig::new(vec![sym, any], sym),
        register_sig,
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
    // `(:use-internals mod)` lowers to this — module privacy's @testable seam
    // (ADR-146): grant this file access to `mod`'s `--` names.
    def(
        heap,
        "%grant-internals",
        Arity::exact(1),
        Sig::new(vec![sym], nil_ty),
        grant_internals,
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
    // The debugger's durable per-process causal context (ADR-174 send-level slice) —
    // `dev-tools` only, so a lean release registers neither (the `debug` module that
    // uses them is a DEV_MODULE too).
    #[cfg(feature = "dev-tools")]
    def(
        heap,
        "%trace-context",
        Arity::exact(0),
        Sig::new(vec![], any),
        trace_context_get,
    );
    #[cfg(feature = "dev-tools")]
    def(
        heap,
        "%set-trace-context",
        Arity::exact(1),
        Sig::new(vec![any], any),
        trace_context_set,
    );
    // The debugger's eval-in-paused-context primitives (ADR-174) — capture a
    // breakpoint's locals and evaluate expressions in that scope. `dev-tools` only.
    #[cfg(feature = "dev-tools")]
    def(
        heap,
        "%locals",
        Arity::exact(0),
        Sig::new(vec![], any),
        locals,
    );
    // `%scope` is the same intrinsic under a name that reads as "capture the lexical
    // scope"; the VM compiles both to the scope map (see `compile_scope_map`), and this
    // registration is the tree-walker fallback (env-frame read) shared with `%locals`.
    #[cfg(feature = "dev-tools")]
    def(
        heap,
        "%scope",
        Arity::exact(0),
        Sig::new(vec![], any),
        locals,
    );
    #[cfg(feature = "dev-tools")]
    def(
        heap,
        "%eval-in",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        eval_in,
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
    // Arg shape: (matcher: callable, timeout: int|nil, tags: vector|nil). The
    // `receive` macro in `std/prelude.blsp` expands to exactly this. The matcher
    // answers `[idx var…]` for the clause that matched (nil = no match); a timeout
    // answers nil. `tags` is the set of leading keywords the clauses can match, or
    // nil to scan everything — a pure filter that lets the scan reject a message by
    // peeking its tag instead of rebuilding it into the heap (see `receive--tags`).
    // `pin` (4th) is the receive-mark hint: the value every clause pins, when they all pin
    // the same one (see `receive--pin`). If it is a `ref` this process minted, the scan can
    // start past every message that predates it — nil disables the hint (ADR-195).
    def(
        heap,
        "%receive",
        Arity::exact(4),
        Sig::new(vec![callable, int.union(nil_ty), any, any], any),
        receive_match,
    );
    // The dirty-native offload pool (ADR-144): the `offload` wrapper in the
    // prelude is the policy; this is the mechanism.
    def(
        heap,
        "%offload",
        Arity::exact(2),
        Sig::new(vec![any, any], int),
        offload_start,
    );
    // MCP progress notifications: a `nest mcp` tool handler reports incremental
    // progress; the dispatcher arms the sink around a progress-token call. A
    // no-op (false) when not under such a call. `mcp-progress` in std/tool/mcp
    // is the friendly wrapper.
    def(
        heap,
        "%mcp-progress",
        Arity::exact(3),
        Sig::new(vec![int, int.union(nil_ty), string.union(nil_ty)], bool_ty),
        mcp_progress,
    );
    // WASM component interop (ADR-071/145, feature `wasm`): the sandboxed
    // native-extension host. Policy — file loading, `use-native` binding —
    // lives in std/wasm.blsp.
    #[cfg(feature = "wasm")]
    {
        def(
            heap,
            "%wasm-load",
            Arity::exact(1),
            Sig::new(vec![any], int),
            wasm_load,
        );
        def(
            heap,
            "%wasm-call",
            Arity::exact(3),
            Sig::new(vec![int, string, any], any),
            wasm_call,
        );
        def(
            heap,
            "%wasm-exports",
            Arity::exact(1),
            Sig::new(vec![int], any),
            wasm_exports,
        );
        def(
            heap,
            "%wasm-close",
            Arity::exact(1),
            Sig::new(vec![int], nil_ty),
            wasm_close,
        );
    }
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
        "process-flag",
        Arity::range(1, 2),
        Sig::new(vec![any, any], any),
        process_flag,
    );
    def(
        heap,
        "hibernate",
        Arity::exact(0),
        Sig::nullary(Ty::of(Tag::Int)),
        hibernate_proc,
    );
    def(
        heap,
        "sched-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        sched_stats,
    );
    def(
        heap,
        "profile-start",
        Arity::range(0, 1),
        Sig::new(vec![any], nil_ty),
        profile_start,
    );
    def(
        heap,
        "profile-stop",
        Arity::exact(0),
        Sig::nullary(list_ty),
        profile_stop,
    );
    def(
        heap,
        "system-monitor",
        Arity::range(0, 2),
        Sig::new(vec![any, any], any),
        system_monitor,
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
        "features",
        Arity::exact(0),
        Sig::nullary(seq),
        features,
    );
    def(
        heap,
        "build-id",
        Arity::exact(0),
        Sig::nullary(string),
        build_id,
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
        // The node name may be a symbol OR a keyword — `expect_node_name` accepts both, and
        // the prelude's `node-start` passes the computed `:name@host` keyword (matching
        // `%node-connect`). Returns the qualified node name, always a keyword.
        Sig::new(vec![sym.union(kw), string, string], kw),
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
        // The peer name may be a symbol OR a keyword — `expect_node_name` accepts
        // both, and the prelude's `connect` passes the computed `:name@host`
        // keyword (matches `register`/`whereis`/`monitor-node`). Returns the
        // authoritative peer name, always as a keyword (`Value::keyword`).
        Sig::new(vec![sym.union(kw), string], kw),
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
        Sig::new(vec![int], bytes_ty),
        random_bytes,
    );
    def(
        heap,
        "%chacha20-encrypt",
        Arity::exact(3),
        Sig::new(vec![any, any, any], bytes_ty),
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
        Sig::new(vec![any, any, int, int], bytes_ty),
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
    // Registered LAST (not beside its scan-tokens sibling) on purpose: registration
    // order feeds the keyword/symbol intern table, and small-map key iteration order is
    // currently downstream of intern ids — an insertion mid-list reshuffles map key
    // order image-wide (record_test's field-order assertion catches it). Appending
    // preserves every existing id. The real fix is insertion-ordered map iteration.
    def(
        heap,
        "scan-form-start",
        Arity::exact(2),
        Sig::new(vec![string, int], int),
        scan_form_start,
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
    ("numerator", &["x"], "The numerator of a ratio (`(numerator 3/4)` → 3), or an integer itself."),
    ("denominator", &["x"], "The positive denominator of a ratio (`(denominator 3/4)` → 4), or 1 for an integer."),
    ("->decimal", &["x"], "A number as an exact base-10 decimal — exact for an integer or terminating ratio (`1/2` → `0.5M`); a non-terminating ratio rounds to the default precision."),
    ("floor", &["x"], "Round x toward negative infinity to an integer. Accepts the whole numeric tower; a ratio floors exactly (not through f64), so it stays correct past 2^53."),
    ("bit-and", &["a", "b"], "Bitwise AND of integers a and b."),
    ("bit-or", &["a", "b"], "Bitwise (inclusive) OR of integers a and b."),
    ("bit-xor", &["a", "b"], "Bitwise exclusive-OR of integers a and b."),
    ("bit-not", &["a"], "Bitwise complement of integer a (two's-complement, so (bit-not n) = (- (- n) 1))."),
    ("bit-shift-left", &["a", "n"], "Shift integer a left by n bits (0 <= n < 64); bits shifted past bit 63 are discarded."),
    ("bit-shift-right", &["a", "n"], "Arithmetic (sign-preserving) right shift of integer a by n bits (0 <= n < 64)."),
    ("bit-count", &["a"], "Population count: the number of 1 bits in integer a's two's-complement representation (a negative a counts its sign bits, so (bit-count -1) = 64). For a bignum it is the popcount of the magnitude."),
    ("bit-positions", &["a"], "A vector of the 0-based bit indices set in non-negative integer a, ascending (e.g. (bit-positions 6) = [1 2]). O(number of set bits) — for a bignum it scans the magnitude. The inverse of summing (bit-shift-left 1 i); handy for enumerating the set bits of an integer."),
    ("float->bits", &["x"], "The IEEE 754 binary64 bit pattern of x, as a non-negative integer (a bignum when the sign bit is set). Reinterpretation, not conversion — the only exact float comparison there is: it separates -0.0 from 0.0 and distinguishes NaN payloads, both of which = collapses. The inverse of bits->float."),
    ("bits->float", &["n"], "The binary64 float whose bit pattern is n (0 <= n < 2^64). The inverse of float->bits."),
    ("cons", &["x", "xs"], "A new pair with head x and tail xs."),
    (
        "first",
        &["coll"],
        "The head of any sequence — a list, vector, bytes, set (an element) or map (a [k v] pair) — or nil if empty.",
    ),
    (
        "rest",
        &["coll"],
        "All but the head of any sequence, as a list (a set yields its remaining elements, a map its remaining [k v] pairs).",
    ),
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
    ("%set", &["&", "xs"], "Build a set from the element args (the programmatic form of the `#{ }` literal). Dedups by structural equality. The `set` library's constructor is Brood over this."),
    ("%set-add", &["s", "x"], "A fresh set like s with element x added (a set already holding x is returned unchanged). O(log n)."),
    ("%set-remove", &["s", "x"], "A fresh set like s with element x removed (absent → unchanged). O(log n)."),
    ("%set-has?", &["s", "x"], "Is x an element of set s? O(log n)."),
    ("%set-count", &["s"], "The number of elements in set s. O(1) — the CHAMP root tracks its size."),
    ("string-length", &["s"], "The number of characters in string s."),
    ("display-width", &["s"], "How many terminal/grid cells string s occupies (grapheme-cluster aware: an emoji / flag / CJK char counts as 2, a combining mark 0). The width-aware counterpart to string-length."),
    ("substring", &["s", "start", "end"], "The characters of s in the range [start, end), char-indexed. end is optional and defaults to (string-length s), so (substring s start) is \"from start to the end\"."),
    ("string-split", &["s", "sep"], "Split s into a list of substrings on each occurrence of sep, in one O(n) pass. An empty separator splits s into its individual characters."),
    ("string->codepoints", &["s"], "The characters of s as a vector of integer Unicode codepoints, in one O(n) pass — the random-access form text parsers index with nth and compare as ints. The inverse of (apply str (map int->char codes))."),
    (
        "grapheme-count",
        &["s"],
        "How many extended grapheme clusters s has — the length a human means, and the exclusive upper bound for grapheme-at. One O(n) pass, no allocation.",
    ),
    (
        "grapheme-at",
        &["s", "i", "default"],
        "The i-th grapheme cluster of s as a string, or default (else nil) when i is out of range. The grapheme-indexed char-at: walks to i instead of materialising every cluster, so a cursor step is not O(line length).",
    ),
    (
        "substring-graphemes",
        &["s", "start", "end"],
        "The half-open grapheme-cluster range [start, end) of s (end optional = to the end), clamped to the ends. The grapheme-indexed substring — plain substring is codepoint-indexed and can slice a cluster in half.",
    ),
    ("string->graphemes", &["s"], "The extended grapheme clusters of s as a vector of strings — the unit a human means by \"character\". \"é\" spelled e + U+0301 is two codepoints but one grapheme; a flag emoji is four codepoints and one grapheme. Step a cursor by this, not by codepoint (which splits clusters and corrupts text). The sibling of string->codepoints; (apply str (string->graphemes s)) is s."),
    ("string-normalize", &["s", "form"], "s in Unicode normalization form, one of :nfc :nfd :nfkc :nfkd. Brood's = is byte-structural, so text that reads identically ('é' as U+00E9 vs U+0065 U+0301) compares unequal until normalized. Canonical (:nfc/:nfd) preserves meaning; compatibility (:nfkc/:nfkd) also folds presentation ('ﬁ' -> 'fi', '²' -> '2') — right for search and identifier matching, wrong for round-tripping text."),
    ("string-span", &["s", "start", "chars"], "The char index just past the maximal run of chars (a set, given as a string) starting at char `start` in s — `start` itself if the char there isn't in the set. The forward char-class scan a tokenizer skips a whitespace/digit run with; O(run) native. See also string-span-until."),
    ("string-span-until", &["s", "start", "chars"], "The char index of the first char of s in the set `chars` (a string) at or after char `start`, or (string-length s) if none — the maximal run of chars NOT in the set. For scanning up to a delimiter (comment-to-newline, atom-to-delimiter). The complement of string-span."),
    ("upper", &["s"], "s upper-cased (Unicode-aware)."),
    ("lower", &["s"], "s lower-cased (Unicode-aware)."),
    ("char->int", &["s"], "Unicode codepoint of the first character of string s (identical to the byte value for ASCII)."),
    ("int->char", &["n"], "A 1-char string for Unicode codepoint n. Errors on an invalid codepoint."),
    ("string->utf8-bytes", &["s"], "The UTF-8 encoding of s as a bytes value."),
    ("utf8-bytes->string", &["bytes"], "Decode UTF-8 bytes (a bytes value, vector, or list of ints 0–255) into a string. Errors on invalid UTF-8."),
    ("to-fixed", &["x", "n"], "Render number x as a string with exactly n digits after the decimal point (rounded). n must be >= 0."),
    ("string->number", &["s"], "Parse s strictly as an int (a bignum when out of i64 range), else a float, else nil (unlike read-string). The inverse of number->string."),
    ("decimal", &["x"], "Construct an exact arbitrary-precision base-10 decimal from x: a string (\"1.50\"), an int (3), a bignum, or a float (converted from its shortest round-trip form, since a float is inexact). For money / Postgres numeric — values a float can't hold exactly. The literal form is a trailing M, e.g. 1.50M."),
    ("decimal->string", &["d"], "The canonical decimal string of decimal d (no M suffix)."),
    ("decimal->float", &["d"], "Decimal d as an (inexact) float."),
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
    ("%f64-sqrt", &["x"], "The IEEE 754 square root of x (f64::sqrt). x must be non-negative; raises otherwise. Handles subnormals and ±0 correctly. Any number coerces to f64 first — int, float, bignum, decimal or ratio."),
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
    ("tls-request", &["host", "port", "request", "ca-pem"], "Make one HTTPS request to host:port (TLS): the response arrives at the calling process as [:tcp sock data] … [:tcp-closed sock] messages (or [:tcp-error sock msg]). request is any iolist (a string, bytes, or nested tree — ADR-141); the socket honors tcp-set-binary for the response. Optional ca-pem (a PEM certificate) replaces the Mozilla roots as the trust anchor — for private CAs and tls-self-signed dev servers. Returns a socket id; pair with tcp-drain. Low-level — prefer http-get."),
    ("tls-listen", &["host", "port", "cert-pem", "key-pem"], "Bind a TLS listening socket on host:port using the PEM certificate chain cert-pem and private key key-pem (port 0 = OS-assigned). Like tcp-listen, connections arrive as [:tcp-accept lsock client]; each accepted socket transparently decrypts inbound to [:tcp …] and encrypts tcp-send, so code above the transport is unchanged. Returns a socket."),
    ("tls-self-signed", &["host"], "Generate a self-signed TLS certificate + private key for host (a DNS name like \"localhost\"), for zero-config dev TLS. Returns [cert-pem key-pem] — pass them to tls-listen. Not for production (clients reject a self-signed cert unless told to trust it)."),
    ("tcp-send", &["sock", "data"], "Write data to sock (blocking). data is any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139). A string leaf is always sent as its UTF-8 bytes, whatever the socket's mode (ADR-141); raw bytes go out as bytes values. Returns nil; throws on error."),
    ("tcp-set-binary", &["sock", "on"], "Switch sock's INBOUND decode between text mode (default) and binary mode; outbound tcp-send is unaffected (ADR-141). In binary mode inbound [:tcp sock data] delivers data as a byte-faithful `bytes` value (not a string) — for length-prefixed / control-byte protocols like WebSocket framing or a database wire protocol. Text mode delivers a UTF-8 string. Returns nil; throws if sock is gone or a listener."),
    ("tcp-set-idle-timeout", &["sock", "ms"], "Arm (or, with ms 0, disarm) an idle timeout on an established stream: the reactor drops the connection if no bytes move in EITHER direction for ms milliseconds, delivering [:tcp-closed] (or [:tcp-error] for a one-shot TLS client). Off by default — arm it on a connection accepting untrusted input as slow-loris protection the reactor applies even if the app forgets to close; leave it off for a legitimately long-idle stream (SSE, long-poll). Returns nil; throws if sock is gone or a listener."),
    ("tcp-controlling-process", &["sock", "pid"], "Make pid the owner of sock's inbound data: starts reading a just-accepted (passive) socket, or retargets an active one. Returns nil."),
    ("tcp-close", &["sock"], "Close sock (a stream or listener), releasing its fd / stopping its accept loop. Idempotent; returns nil."),
    ("tcp-local-port", &["sock"], "The local port sock is bound to, or nil."),
    ("proc-spawn", &["prog", "args", "opts"], "Spawn prog (a string) with args (a list/vector of strings) as a persistent child process with piped stdio. An optional opts map tunes the child: :cwd (a string) sets its working directory, :env (a map of string->string) adds environment variables on top of the inherited environment. Its stdout/stderr arrive at the calling process as [:proc handle data] / [:proc-err handle data] messages, and [:proc-closed handle code] on exit (code is the exit status, or nil if signalled). Returns a subprocess handle. Throws if prog can't be spawned."),
    ("proc-send", &["p", "data"], "Write data to subprocess p's stdin (blocking) and flush. data is any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139); a string leaf is always its UTF-8 bytes, whatever the child's mode (ADR-141). Returns nil; throws if p is unknown/closed."),
    ("proc-set-binary", &["p", "on"], "Switch subprocess p's INBOUND decode between text mode (default) and binary mode (mirrors tcp-set-binary; outbound proc-send is unaffected, ADR-141). In binary mode inbound [:proc …]/[:proc-err …] delivers data as a byte-faithful `bytes` value (not a string) — for a child speaking a binary protocol over stdio. Returns nil; throws if p is unknown/closed."),
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
    ("check-file", &["path", "&optional required-mods"], "Advisory type-check every top-level form in the file at path: a list of `path:line:col: warning: …` strings, or nil. Does not evaluate the file. `required-mods` is the file's transitive require-closure (module-name strings) — the KI-17 reachability set that flags a qualified `mod/name` whose module the file never requires; omit it (single-file / editor) to disable that lint."),
    ("check-file-structured", &["path", "&optional required-mods"], "Like check-file but returns a list of `{:file :line :col :message}` maps instead of GNU-format strings — for tools (the `nest mcp` `check` tool, editor diagnostics). `required-mods`: see check-file."),
    ("check-file-deps", &["path", "&optional required-mods"], "Incremental-cache check (ADR-119): returns [warnings dep-keys fingerprint] — the GNU warning strings, the set of global observations the check made, and a fingerprint of them against the current image. Store dep-keys+fingerprint; reuse warnings on a later run iff (check-deps-fp dep-keys) still matches and the file's mtime is unchanged. `required-mods`: see check-file."),
    ("%module-direct-requires", &["path"], "Parse the file at path (no eval) and return `{:module <name-or-nil> :requires [<module-name> …]}` — its own module name and the modules it directly `:use`s / `:use-internals` / `(require 'M)`s. The edge list `project.blsp` closes transitively into each file's check-file reachability set (KI-17)."),
    ("check-deps-fp", &["dep-keys"], "Recompute the fingerprint of a file's dep-keys (from check-file-deps) against the current global image. The incremental check cache reuses a file's warnings iff this equals the stored fingerprint."),
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
    ("gc-stats", &[], "A snapshot map of GC activity: :collections, :copied, :reclaimed (cumulative object counts), :live, :live-bytes, :threshold (next-collection trigger), and the pause-duration trio :pause-total-us/:pause-max-us/:pause-last-us (cumulative wall time in collections, worst single pause, most recent — the timing tier) for the caller's own LOCAL heap; :runtime-closures and :runtime-threshold for the *shared* RUNTIME code region (its promoted-closure count + next auto-compact trigger — same for every process); and :debug-build (true if built with debug assertions — not a perf build). The LOCAL figures are per-process; use (runtime-collect) for the RUNTIME live/reclaimable split."),
    ("vm-stats", &[], "A snapshot map of VM work-attribution counters (the perf-stats feature). :enabled is false unless the binary was built with --features perf-stats; when true, process-global cumulative totals: :vm-apply (closure activations), :tail-call/:self-tail (trampoline iterations), :tw-defer (tree-walker fallbacks), :call-ic-hit/:call-ic-miss, :global-ic-hit/:global-ic-miss, :prim2-inline/:prim2-fallback, :prim1-inline/:prim1-fallback, :env-get/:env-hops (lookups + chain frames walked), :alloc (LOCAL allocations). Tells you whether the VM is dispatch-, env-, or alloc-bound. A counting tool, not a timing one — read times from the benches (docs/benchmarking.md)."),
    ("gc-collect", &[], "Force a collection of this process's LOCAL heap now, returning the post-collection gc-stats map. An observability/test aid, not a load-bearing trigger — automatic collection at the eval safepoint already keeps memory bounded."),
    ("hibernate", &[], "Tell the runtime this process is about to idle for a long time: collect, shrink its heap slabs and root vectors, and drop its inline caches and compiled-body cache. Returns the bytes of slab capacity released. Erlang's erlang:hibernate/3 (minus the continuation argument — Brood processes resume from their receive). Use it in a process that will wait a long while (a pooled connection, an idle session actor) — it trades a one-off cache rebuild on the next call for a substantially smaller idle footprint. Do NOT use it in a request loop: dropping the caches per park costs message-heavy code 12-26%, which is exactly why this is an explicit call and not automatic."),
    ("runtime-collect", &[], "Compact the shared RUNTIME code region, reclaiming superseded versions of redefined globals (hot-reload churn). Returns {:before N :after M :reclaimed (N-M) :ran bool} (closure counts). Runs only when this runtime is uniquely owned (no other live process) — otherwise :ran is false and nothing changes. Usually unnecessary: the eval safepoint auto-compacts once hot-reload churn crosses a threshold (single-process); this forces it now. ADR-076 follow-up / docs/runtime-collector-exploration.md."),
    ("gc-trace", &["on?"], "Query (no arg) or set (truthy arg) per-collection GC trace logging for this process; returns the resulting state. When on, each minor/major collection prints a one-line summary to stderr. Defaulted from BROOD_GC_TRACE."),
    ("eval", &["form"], "Evaluate a form in the global environment."),
    ("read-string", &["s"], "Parse and return the single form in string s. Errors on trailing content after the form (rather than silently dropping it) — use read-all for input with more than one form."),
    ("read-all", &["s"], "Parse every form in string s and return them as a list (the all-forms sibling of read-string)."),
    ("read-first", &["s"], "Parse and return the first form in string s, ignoring any trailing forms (the lenient sibling of read-string — for peeking a multi-form source's leading form, e.g. a file's (defmodule …) header)."),
    ("parse-source", &["s"], "Parse s into a lossless CST tree as nested vectors (mechanism for std/format.blsp)."),
    ("scan-source-extract", &["src"], "Native per-file scan for the whole-project check (ADR-119): parse src and return [counts privs def-names] — a map of --containing symbol counts, this file's --private defs as [bare qual], and every top-level def's qualified name. The fast path replacing the interpreted CST walk."),
    ("scan-tokens", &["s"], "Lexically tokenize Brood source s into a vector of [start end kind text] tokens (char offsets, end-exclusive; whitespace skipped). kind is :comment, :string, :number, :keyword, :symbol, :open, or :close. The lossless token stream a fontifier / structural tool walks — the per-char scan runs natively, leaving policy (faces, head-position) to the consumer over O(tokens)."),
    ("scan-form-start", &["s", "pos"], "The greatest char offset <= pos of a column-0 open bracket in s lying OUTSIDE any string or ; comment, else 0 — the string/comment-aware beginning-of-defun behind highlight/safe-restart and tool/sexp narrowing. The required forward lexical pass (a backward scan cannot know string state) runs natively: O(pos) at native speed instead of interpreted per-char cost on every eldoc/fontify-restart in a large buffer."),
    ("span-runs", &["text", "base", "spans", "ranges"], "Tile text (first char at offset base) into a list of [substring face] runs from ascending, non-overlapping [start end face] spans: gaps are nil-faced, each span its text in its face. With optional overlay ranges ([lo hi face], may overlap/be unordered) each char's face is its span face with every covering range face merged on top (later wins). Adjacent equal-face runs coalesce. The highlight span->runs tiler (fontify-runs), in Rust. Faces are opaque maps."),
    ("clipboard-get", &[], "The OS clipboard's text, or nil when empty / non-text / unavailable (no display server, or a build without the clipboard feature)."),
    ("clipboard-set!", &["s"], "Copy string s to the OS clipboard so other apps can paste it; returns s. A no-op (still returns s) when no clipboard is available or the clipboard feature is off."),
    ("parse-source-positioned", &["s"], "Parse s into a CST of maps, each `{:kind :start :end}` (leaves add :text, containers/wrappers add :kids) with half-open character offsets — for structural navigation (std/sexp)."),
    ("tree-sitter-parse", &["source", "lang"], "Parse source (a string) with the tree-sitter grammar named by keyword lang into a positioned CST — the SAME node-map shape as parse-source-positioned (`{:kind :start :end :named}`; leaves add :text, nodes with children add :kids), :kind a keyword of the tree-sitter node type and :named false for anonymous tokens (keywords/punctuation). Char offsets, so std/sexp + the editor's fontify navigate it unchanged. The generic mechanism is in the default build, but the kernel ships NO language grammar — a grammar is opt-in (e.g. --features treesit-ruby, or treesit-grammars for all). Errors if the named language's grammar isn't built in, or if the runtime was built without --features treesit."),
    ("tree-sitter-reparse", &["key", "source", "lang"], "Like tree-sitter-parse, but incremental: caches the last (source, tree) for integer buffer id `key` and re-uses it (deriving the edit by diffing the old source) so only the changed region is re-scanned. Same positioned CST as tree-sitter-parse — incrementality is a pure optimization. Identical source re-uses the cached tree with no re-parse. Call tree-sitter-forget when the buffer closes."),
    ("tree-sitter-forget", &["key"], "Drop every cached incremental tree for integer buffer id `key` (all languages); returns the count dropped. Call when a buffer closes so tree-sitter-reparse's cache can't grow unbounded."),
    ("eval-string", &["s"], "Read and evaluate every form in string s (the string analogue of load)."),
    ("load", &["path"], "Read and evaluate every form in the file at path."),
    ("%run-program-file", &["path"], "Run the program file at `path` as its own green process (ADR-135) and block until it finishes; nil, or raises if a top-level form did. Unlike `load` (which tree-walks inline, so a top-level `receive` blocks the caller), the file runs on a worker in capture mode — top-level `receive`s park-and-capture and message-passing uses the userspace direct-handoff path. Shares this runtime's globals/`*load-path*`. `nest run FILE` routes here."),
    ("reload-defs", &["path"], "Re-evaluate only the def-style top-level forms in `path` (def, defn, defmacro, defmodule, defdyn, …) — skipping other top-level calls. Used by file watchers to refresh code without re-running side-effecting top-level calls like a `(main-loop)`. Returns nil."),
    ("apply", &["f", "&", "args"], "Call f with the leading args plus the final list argument spliced in as trailing args."),
    ("name", &["x"], "The spelling of a symbol or keyword as a string (no leading colon)."),
    ("symbol", &["x"], "Coerce a string, symbol, or keyword to the matching symbol (interning if needed)."),
    ("keyword", &["x"], "Coerce a string, symbol, or keyword to the matching keyword (interning if needed)."),
    ("cwd", &[], "The current working directory."),
    ("file-exists?", &["path"], "Whether path exists."),
    ("canonicalize", &["path"], "The real absolute path of `path` with symlinks and ./.. resolved. Works for a not-yet-existing target (the longest existing ancestor is resolved, then the remaining components appended). Relative paths are taken against the cwd. nil only if the cwd itself can't be read. Use it to make path sandboxing symlink-escape-proof."),
    ("dir?", &["path"], "Whether path is a directory."),
    ("list-dir", &["path"], "The entry names directly under directory path, sorted."),
    ("make-dir", &["path"], "Create a directory and any missing parents (like mkdir -p)."),
    ("spit", &["path", "s"], "Write s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, replacing any existing file."),
    ("spit-append", &["path", "s"], "Append s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, creating it if absent (unlike spit, which truncates). Returns nil. Opens in append mode so each write lands at end-of-file — the OS-atomic append that makes a log safe to write from several processes at once. The string sibling of append-bytes."),
    ("%file-swap", &["lock-path", "data-path", "expected", "new"], "Replace the entire contents of data-path with new, but ONLY if they currently equal expected; returns true when swapped, false when they differ (re-read, recompute, retry). Serialised across processes by a blocking exclusive lock on lock-path (a separate file — the data file is replaced by rename, so a lock on it would exclude nobody), and crash-atomic (temp file + rename, so a crash leaves the old contents intact). A missing data-path reads as \"\", so the same call creates it. The mechanism behind a safe read-modify-write whose modify step is Brood code."),
    ("spit-private", &["path", "s"], "Write string s to path with owner-only (0600) permissions, creating the parent dir if needed. The private-by-default write for a secret (spit leaves a world-readable file)."),
    ("slurp", &["path"], "Read the whole file at path into a string (does not evaluate it). UTF-8; throws on a non-text file — use slurp-bytes for binary."),
    ("slurp-bytes", &["path"], "Read the whole file at path as a bytes value. The byte-faithful read slurp can't be (slurp is UTF-8 and throws on a non-text file). Pairs with hash/sha256-bytes / hash/sha256-raw and the encoding byte variants — e.g. hashing a binary asset."),
    ("spit-bytes", &["path", "bytes"], "Write any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) to path byte-faithfully, replacing any existing file. Returns nil. The binary write-side counterpart to slurp-bytes (spit is UTF-8 string-only) — materialises a received image / archive / any binary asset to disk."),
    ("append-bytes", &["path", "bytes"], "Append any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) to the file at path byte-faithfully, creating it if absent. Returns nil. The incremental counterpart to spit-bytes (which truncates) — lets a large payload be streamed to disk chunk-by-chunk (e.g. spooling a file upload) without ever holding it whole in memory."),
    ("random-token", &["n"], "n cryptographically-strong random bytes from the OS RNG, hex-encoded as a 2n-char string. Used to mint a node cookie."),
    ("%digest", &["algo", "bytes"], "Raw digest of a byte sequence (bytes value, vector, or list of byte ints 0–255) under algorithm keyword `algo` (:md5 :sha1 :sha256 :sha384 :sha512), returned as a bytes value (not hex). The one digest primitive; the public sha256/md5/… hex/string names are Brood over this in std/hash.blsp."),
    ("%offload", &["f", "args"], "Run the blocking native `f` with `args` (a vector) on the dirty-offload OS pool (ADR-144) instead of this process's scheduler worker. Returns a token int immediately; the pool later delivers [:offload token result] or [:offload-error token err] to the calling process's mailbox. Only long/blocking data-in/data-out natives are allowed (%git-clone, %git-resolve-ref, %pbkdf2-sha256-bytes, %digest, %hmac, slurp, slurp-bytes, spit, spit-bytes, spit-append, append-bytes, tls-self-signed) — anything heap-sharing or env-reading is refused. Prefer the prelude `offload` wrapper, which parks in a selective receive and rethrows errors."),
    ("%hmac", &["algo", "key-bytes", "msg-bytes"], "HMAC of `msg-bytes` keyed by `key-bytes` (both byte sequences) under algorithm keyword `algo` (:md5 :sha1 :sha256 :sha384 :sha512), returned as a bytes value (raw MAC, not hex). The public hmac-sha256/… names are Brood over this in std/hash.blsp."),
    ("%git-resolve-ref", &["url", "ref"], "Resolve git `ref` (tag/branch/commit) at remote `url` to a commit hash (via `git ls-remote`), or nil if not found. The package manager's ref-pinning mechanism (ADR-037)."),
    ("%git-clone", &["url", "dest", "ref", "commit"], "Shallow-clone `url` into `dest` and check out the exact `commit` (detached); `ref` is the fetch fallback. Returns :ok or throws. The package manager's fetch mechanism (ADR-037)."),
    ("%git-changed-files", &["dir"], "Absolute paths of files NOT committed-clean under `dir` (modified, staged, or untracked — the union `git status --porcelain` reports). Returns a list of strings (nil when the tree is clean — an empty list is nil), or the keyword :not-a-repo when `dir` is not inside a git work tree. Backs `nest format --changed`."),
    ("%rm-rf", &["path"], "Recursively delete `path`. Bounded to paths under `_deps/` (refuses anything else). Idempotent. The package manager's cache-eviction mechanism (ADR-037)."),
    ("%untar-gz", &["archive", "dest", "strip"], "Extract a gzip'd tar `archive` into `dest`, stripping `strip` leading path components (package convention: 1). Shells to `tar`. Returns :ok or throws. The tarball-dep delivery mechanism (ADR-037)."),
    ("read-line", &[], "Read one line from stdin; returns the line as a string (trailing newline stripped) or nil at end of input."),
    ("file-mtime", &["path"], "Last-modified time of path as epoch-milliseconds, or nil if the file is missing. Cheap (stat) — pair with `load` to drive a hot-reloader."),
    ("file-size", &["path"], "Size of the file at path in bytes, or nil if it is missing."),
    ("file-stat", &["path"], "Metadata for path in ONE stat as a map {:dir? :size :mtime :atime :symlink? :exec? :mode :nlink :uid :gid :owner :group}, or nil if missing. :symlink? reads the link itself (lstat); the rest follow it. :mtime/:atime are epoch-ms last-modified/last-access (nil if unreadable; :atime may be coarse under relatime/noatime mounts); :exec? is the owner-execute bit; :mode is the unix permission bits (0 off-unix); :nlink the hard-link count; :uid/:gid the numeric ids; :owner/:group their resolved names (the numeric id as a string if unresolved). Everything an `ls -l` row + a recency sort needs in one syscall."),
    ("delete-file", &["path"], "Remove the file at path. Idempotent (nil if already absent); errors on a real I/O failure."),
    ("delete-dir", &["path"], "Remove a directory and everything under it (recursive). Idempotent (nil if already absent); errors on a real I/O failure."),
    ("rename-file", &["from", "to"], "Rename/move file `from` to `to`. Returns nil; errors on failure."),
    ("copy-file", &["from", "to"], "Copy file `from` to `to` (replacing `to`), preserving contents and permissions. Binary-safe (unlike slurp+spit). Returns nil; errors on failure."),
    ("image-thumb", &["bytes", "max-w", "max-h"], "Decode an encoded image (PNG/JPEG/GIF/WebP/BMP) from a byte sequence and downscale it to fit within max-w×max-h pixels (aspect ratio preserved), returning {:width :height :rgba} where :rgba is a width*height*4 bytes value (row-major RGBA8). nil when the bytes aren't a decodable image or the dims are non-positive. Per-call decode limits bound a decompression bomb. The one image primitive; rendering (half-block cells, a GUI texture) is Brood policy over the decoded buffer."),
    ("getenv", &["name"], "The value of environment variable name, or nil if unset."),
    ("hostname", &[], "This machine's short hostname (no domain). Used to qualify a node name as name@host."),
    ("%install-interrupt-handler", &[], "Take over SIGINT so Ctrl-C records a request instead of terminating the runtime; returns true when installed (false with no Unix signals). Idempotent, and clears any pending request. Opt-in, so a script keeps dying on Ctrl-C: the REPL installs it, nothing else does."),
    ("%restore-interrupt-handler", &[], "Restore the default SIGINT disposition (Ctrl-C terminates again) and clear any pending request — the uninstall half of %install-interrupt-handler, so a transient REPL (pry) inside a script gives the script its Ctrl-C back. Returns true when restored."),
    ("%interrupt-taken?", &[], "True if an interrupt arrived since the last call, clearing it (read-and-clear, so one Ctrl-C is acted on once). Poll this while a spawned evaluation runs and (exit pid :kill) it."),
    ("run-process", &["prog", "args"], "Run external program prog with an args list, inheriting stdio; returns its exit code."),
    ("%env-all", &[], "All environment variables as a map of string→string."),
    ("%argv", &[], "Command-line arguments as a vector of strings (including argv[0])."),
    ("%os-type", &[], "The host OS as a keyword: :linux, :macos, or :windows."),
    ("%os-cmd", &["prog", "&", "args"], "Run prog (with optional args list) capturing stdout/stderr; returns {:stdout s :stderr s :exit n}."),
    ("%halt", &["code"], "Terminate the process with exit code. Never returns."),
    ("%random-bytes", &["n"], "n cryptographically-strong random bytes as a bytes value."),
    ("%chacha20-encrypt", &["key-bytes", "nonce-bytes", "plaintext-bytes"], "Encrypt plaintext-bytes with ChaCha20-Poly1305 (AEAD). key-bytes must be 32 bytes; nonce-bytes must be 12 bytes. Returns ciphertext bytes (plaintext + 16-byte auth tag). NEVER reuse a (key, nonce) pair — use a fresh nonce per message (see crypto/random-nonce)."),
    ("%chacha20-decrypt", &["key-bytes", "nonce-bytes", "ciphertext-bytes"], "Decrypt ciphertext-bytes with ChaCha20-Poly1305. Returns plaintext bytes, or :error if authentication fails."),
    ("%pbkdf2-sha256-bytes", &["password-bytes", "salt-bytes", "iterations", "key-len"], "PBKDF2-HMAC-SHA256 key derivation over byte-sequence password and salt (raw bytes, not UTF-8 strings — a binary salt round-trips faithfully). Returns a key-len-byte bytes value. Use iterations >= 600000 for password storage."),
    ("macroexpand-1", &["form"], "Expand form by a single macro step."),
    // `macroexpand` is a Brood prelude fn (ADR-064), documented via its docstring.
    ("gensym", &["prefix"], "A fresh, unique symbol, with an optional name prefix."),
    ("form-pos", &["form"], "A form's [line col] source position, or nil."),
    ("current-file", &[], "The path of the file currently being loaded, or nil."),
    ("source-location", &["name"], "Where global name was defined, as [file line col], or nil. Quote it: (source-location 'foo)."),
    ("type-signature", &["name"], "The checker's type signature for global `name` (declared/curated/inferred) as an arrow string like \"(int -> int)\", or nil if it can't be pinned. Symbol or string arg: (type-signature 'map)."),
    ("references-in-source", &["name", "source"], "Occurrences of the global `name` in `source`, as a list of [line col] (1-based); locals that shadow it are excluded."),
    ("doc", &["f"], "The docstring of a function, macro, or primitive, or nil."),
    ("arglist", &["f"], "The parameter list of a function, macro, or primitive, or nil."),
    ("global-names", &[], "Every globally bound symbol, sorted by spelling."),
    ("%coverage-lines", &[], "Every source line recorded as EXECUTED, as a list of [file (line …)]. Empty unless the run was started with BROOD_COVERAGE=1 (`nest test --cover-lines`)."),
    ("%coverage-precompile", &["f"], "Compile f's body now, without calling it, so its lines count toward %coverage-instrumented. Returns true if a body was compiled."),
    ("%coverage-instrumented", &[], "Every source line the compiler INSTRUMENTED, as a list of [file (line …)] — the denominator %coverage-lines is a subset of. Arms compile when defined, so a never-called function appears here and not there."),
    ("%coverage-branches", &[], "Every branch edge recorded as taken, as [file ([line col taken] …)]. A branch is fully covered when both edges (taken true and false) appear for one [line col]. Empty unless BROOD_COVERAGE=1 (`nest test --cover-branches`)."),
    ("%coverage-branch-instrumented", &[], "Every [line col] branch point the compiler INSTRUMENTED, as [file ([line col] …)] — the branch denominator (each needs both edges taken for full coverage)."),
    ("%coverage-reset", &[], "Forget every line recorded by %coverage-lines, so a long-lived image can measure more than once without runs bleeding together."),
    ("builtin-modules", &[], "The names of every module baked into this binary, as a sorted list of strings — what `(require 'name)` resolves without a load-path. Backs `nest` shell completion and lets a name be validated before requiring it."),
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
    ("mailbox-size", &["pid"], "How many messages are queued in pid's mailbox (its receive backlog), or nil if pid is not a live local process. The one process-introspection accessor not reachable from Brood; see std/tool/observer.blsp."),
    ("process-info", &["pid"], "A snapshot map of a live local process: {:id :pid :node :name :status :mailbox :monitored-by :parent :memory :collections :reductions} (:pid the process's pid value, for acting on it with exit/send/monitor; :status is :running or :waiting; :name nil if unregistered; :parent the spawner's id, nil for the root; :memory the LOCAL heap bytes and :collections the cumulative GC count, both as of the process's last receive; :reductions the cumulative reduction count — Erlang's scheduling unit, updated every quantum; exact for spawned processes, coarse for the root). nil for a remote/dead pid. The Erlang-process_info-style introspection the observer reads; see std/tool/observer.blsp."),
    ("term-enter", &[], "Enter raw mode + the alternate screen, hide the cursor, and enable mouse capture, taking over the terminal for a full-screen UI (so click/scroll reach term-poll). Pair with term-leave. (ADR-046 display seam.)"),
    ("term-leave", &[], "Restore the terminal: show the cursor, disable mouse capture, leave the alternate screen, disable raw mode. The normal-path teardown for term-enter."),
    ("term-size", &[], "The terminal size as [cols rows] in character cells."),
    ("term-poll", &["ms"], "Wait up to ms milliseconds for an input event; return a key (a 1-char string for printables, or a keyword for specials: :up :down :left :right :enter :escape :backspace :tab :back-tab :delete :home :end :page-up :page-down, ctrl combos like :ctrl-c, alt combos like :alt-f), a mouse event as a vector [:mouse action button row col mods] (action: :press :release :drag :scroll-up :scroll-down — :drag is motion with a button held, reported once per cell crossed; button: :left :right :middle or nil for scroll; row/col 0-based cells; mods a vector of held modifier keywords in :ctrl :alt :shift order, [] when none — so Ctrl+wheel etc. are bindable), or nil on timeout. Always pass a finite ms."),
    ("term-draw", &["frame"], "Paint a frame — a vector of render ops: [:clear], [:text row col str], [:text row col str face], [:rect row col w h face], [:cursor row col] / [:cursor row col style]. A face is a map like {:fg :red :bold true}; a colour is a palette keyword (:red … :dark-grey, the terminal's named colour) or an explicit [r g b] vector / \"#rrggbb\" hex string (a true-colour cell). [:rect …] fills a w×h cell block with the face's background (a solid panel). The optional cursor `style` is :block (default), :bar, or :underline — the steady caret shape. The in-process frontend for the display protocol; returns nil."),
    ("gui-open", &["title?", "width?", "height?", "opts?"], "Open a new native window and return its integer id (needs the runtime built with --features gui; errors otherwise). An optional `title` string sets the OS title-bar text (default `Brood`); change it later with gui-title!. Optional `width` `height` (logical pixels, both required together) set the initial window size (default 840x560). Optional `opts` map, the attributes fixed when the window is built: `{:decorations false}` opens a **borderless** window — no OS title bar or frame — for an app that draws its own chrome (a browser's tab strip and toolbar) and would otherwise sit under a redundant second title; `{:app-id \"my-app\"}` sets the desktop application id (Wayland `app_id`, X11 `WM_CLASS`), which the desktop matches against the installed `my-app.desktop` entry to give the window its real icon and name in the dash / alt-tab — without one it is unidentifiable and draws the desktop's generic fallback icon (on Wayland a client cannot supply icon pixels at all, so this, not gui-icon!, is how a window gets an icon there). Its key/mouse input is delivered to the CALLING process's mailbox as messages — a key as a 1-char string / keyword (`:up`, `:ctrl-c`), the mouse as `[:mouse action button row col mods]` (action `:press`/`:release`/`:drag`/`:move`/`:scroll-up`/`:scroll-down` — `:drag` is motion with a button held and `:move` is bare motion with none (button nil), both delivered once per cell crossed (so mouse-look / hover need no click); `mods` a vector of held modifier keywords in `:ctrl :alt :shift` order, `[]` when none, so Ctrl+wheel / Ctrl+drag are bindable; a `:press` carries a trailing 7th element, its click-chain count `[… mods n]` — 1 single, 2 double, 3 triple, … for repeated presses of the same button in the same cell within the double-click window, so double-click-to-select-word and triple-click-to-select-line are bindable; the terminal reports 1), a resize as `[:resize cols rows]` (the new cell grid, so the loop re-renders at the new size) — so the consumer parks in `(receive)` instead of polling (ADR-058). Clicking the window's close button delivers a dedicated `:close` message — distinct from the Escape *key* (`:escape`), so an app can quit on the X without conflating it with Escape (which an editor binds to cancel/normal-mode); `ui-run` quits on `:close` automatically. Starts the GUI thread on the first call; each call is an independent window, so several observers can run at once. Pass the id to the other gui-* primitives; pair with gui-close."),
    ("gui-close", &["id"], "Close window id (the teardown for gui-open). Idempotent; an unknown id is a no-op."),
    ("gui-title!", &["id", "text"], "Set window id's OS title-bar text to the string text at runtime (the title gui-open gave it, or the default, otherwise). Needs --features gui; a no-op if the GUI thread never started or id isn't a live window. Returns nil."),
    ("gui-icon!", &["id", "rgba", "w", "h"], "Set window id's taskbar / title-bar icon from raw RGBA pixels: rgba is a vector of w*h*4 byte ints (0-255), row-major, 4 per pixel (red, green, blue, alpha). Needs --features gui; a silent no-op if the GUI thread never started, id isn't a live window, or the data length isn't w*h*4. Where the OS shows it depends on the platform (X11/Windows use it directly; Wayland prefers a .desktop file). Returns nil."),
    ("gui-focus", &["id"], "Raise window id to the front and give it OS keyboard focus, un-minimising it first. Lets an app surface an already-open (singleton) window instead of opening a duplicate — e.g. `(observe)` focuses its existing window rather than spawning a second. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-grab-cursor", &["id", "on"], "Confine the pointer to window id while `on` is truthy, release it otherwise — for mouse-look that shouldn't let the cursor slip out of the window and click another app. Uses the platform's `Confined` grab (cursor stays inside but keeps moving, so an absolute position-based look maps edge-to-edge), falling back to `Locked` where that's all the platform offers. Off by default; an app opts in. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-fullscreen!", &["id", "on"], "Make window id borderless-fullscreen while `on` is truthy (covering the whole monitor it's on, NO title bar / decorations — distraction-free), or restore it to a normal window otherwise. For a big-but-normal window that keeps its title bar, use gui-maximize! instead. The fullscreen/restore triggers a resize, so the consumer gets the usual [:resize cols rows] message and re-renders at the new size. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-maximize!", &["id", "on"], "Maximise window id while `on` is truthy (fill the screen's work area, KEEPING the title bar / decorations), or restore it to its previous size otherwise — e.g. an editor's init file opening big without going true-fullscreen. The maximise/restore triggers a resize, so the consumer gets the usual [:resize cols rows] message and re-renders at the new size. Errors only if id isn't a live window. Needs --features gui. Returns nil."),
    ("gui-minimize!", &["id"], "Iconify window `id`. The counterpart of gui-maximize! for an app that draws its own window controls, which a borderless window (gui-open with {:decorations false}) must."),
    ("gui-drag-move", &["id"], "Hand window `id` to the window manager for an interactive move, for the rest of the currently-held press. What a borderless window needs to stay movable: with no OS title bar there is nothing to grab, so the app nominates a region of its own chrome (a browser's tab strip) and calls this when a press lands there. A platform that declines the gesture is a no-op, not an error."),
    ("gui-drag-resize", &["id", "dir"], "Hand window `id` to the window manager for an interactive resize from `dir` — :north :south :east :west :north-east :north-west :south-east :south-west. The window-frame counterpart of gui-drag-move, for a borderless window that draws its own edges. A platform that declines the gesture is a no-op, not an error."),
    ("gui-size", &["id"], "Window id's size as [cols rows] in character cells (tracks resize / HiDPI), same shape as term-size."),
    ("audio-beep", &["freq-hz", "ms", "vol"], "Play a short tone of freq-hz for ms milliseconds, optionally at peak amplitude vol (0..1, default ~0.18 — pass a small vol for quiet/ambient sounds). Fire-and-forget — it never blocks the caller, and overlapping beeps mix — so a game can blip from its frame loop. Synthesised on a dedicated audio thread (needs --features audio). A graceful no-op without the feature, when there's no audio device, or when muted via BROOD_AUDIO=0 or BROOD_GUI_HEADLESS. Returns nil."),
    ("gui-held-key", &["id"], "The key window id currently sees as physically held — the same value its press delivered (a 1-char string, or a keyword like :ctrl-n / :up) — or nil when none is held. Tracked from press/release transitions in the event loop (NOT winit's ke.repeat, unreliable on Wayland), so it's the source of truth for a held key: a consumer-paced auto-repeat polls it each tick and stops the instant it no longer matches, so a missed key-up (e.g. lost on focus change) can't cause runaway repeat."),
    ("gui-draw", &["id", "frame"], "Paint a frame (the same render-op vector term-draw takes) to window id; returns nil. Unknown ops are skipped (forward-compatible). A text op's face may carry :scale n (GUI only, integer >=1, capped at 16): the text is drawn n× larger in an n×n block of cells anchored at its row/col — the per-pane/per-buffer font knob; the terminal frontend renders scale 1. A `[:cursor row col]` op may carry an optional `style` keyword (`[:cursor row col style]`) — :block (default, a 50% overlay), :bar (a thin caret on the cell's left edge), or :underline (a rule along the cell bottom). A `[:rect row col w h face]` op fills a w×h cell block with the face's background colour — a solid panel painted directly (no glyphs), the multi-row generalisation of a status bar. A `[:cursor-zone x y w h shape]` op marks a hover hot-zone: while the pointer is over it the window shows the resize cursor `shape` (:col-resize ↔ / :row-resize ↕), hit-tested on the GUI thread (ADR-080); it draws nothing and the terminal ignores it. A `[:vspans row0 col0 cols]` op is the column-renderer fast path (raycasters, spectrum bars): `cols` is a vector with one entry per cell-column (`col0`, `col0+1`, …), each a top-to-bottom stack of `[height colour]` segments painted from `row0` down — `colour` a face keyword (`:red`), an `[r g b]` triple (0..255), or nil (transparent). The per-cell fill happens natively here, so a wide scene costs the Brood side O(columns), not O(cells); GUI-only (the terminal ignores it)."),
    ("gui-font!", &["id?", "spec"], "Set a cell font from spec, a map {:family <keyword> :height <px>} (both keys optional): :family picks a registered font family (bundled :mono, or one added by gui-font-register), :height the cell pixel size. (gui-font! spec) sets the global default — every open window and ones opened later; (gui-font! id spec) retunes just window id, leaving the global default and other windows alone, so two windows can run different fonts. Per-section fonts within a window come from a face's :family/:scale. Needs --features gui. Returns nil."),
    ("gui-inset!", &["px"], "Set the window content inset to px logical pixels: a blank margin before the cell grid on every window edge, so a GUI app's text breathes instead of sitting flush against the frame. Applies to every open window and the default for ones opened later; the grid loses 2*px per axis (fewer cells) and re-renders. The inset is shared by the renderer and mouse hit-testing, so clicks stay aligned. Needs --features gui. Returns nil."),
    ("gui-bg!", &["color"], "Set the window background colour: the fill for :clear, the per-frame pre-clear, and — being outside every cell — the gui-inset! margin and the cell-grid snap remainder. So a GUI app's padding matches its own theme background instead of the hardcoded default. color is a keyword named colour, an [r g b] vector (0..255 per channel), or a \"#rrggbb\"/\"#rgb\" hex string; nil restores the default. Applies to every open window and the default for ones opened later (a pure repaint — no grid change). Needs --features gui. Returns nil."),
    ("gui-font-register", &["name", "styles"], "Register font family name (a keyword) from styles, a map of style → TTF file path {:regular \"…\" :bold \"…\" :italic \"…\" :bold-italic \"…\"}. Only :regular is required; a missing style reuses the regular file. Afterwards a face's :family <name> (or gui-font!) selects it. Needs --features gui. Returns name."),
    ("term-raw-enter", &[], "Enter raw mode only — NO alternate screen, cursor stays visible, scrollback preserved. The seam for an inline line editor (the REPL); use term-enter instead for a full-screen TUI. Pair with term-raw-leave."),
    ("term-raw-leave", &[], "Leave raw mode (the teardown for term-raw-enter). Idempotent with the panic-path restore."),
    ("term-emit", &["ops"], "Paint inline, relative-motion render ops (for an in-place editor that must not take over the screen): [:print str], [:print str face], [:cr], [:nl], [:up n], [:down n], [:col n], [:clear-eol], [:clear-below], [:clear-screen]. A face is a map like {:fg :cyan :bold true}. Queues all ops then flushes once; unknown ops are skipped; returns nil."),
    ("demonitor", &["mref"], "Drop the monitor identified by mref (best-effort)."),
    ("link", &["pid"], "Symmetrically link the current process and pid, local or remote (Erlang link/1). When either dies, the other gets a [:EXIT pid reason] message if it set (trap-exit true), else dies too on an abnormal reason (propagation cascades through links; :normal does not propagate). A remote link fires :noconnection on net-split; linking an already-dead/unreachable pid notifies the caller (:noproc / :noconnection). Returns nil."),
    ("unlink", &["pid"], "Drop the symmetric link between the current process and pid (local or remote; best-effort). Returns nil."),
    ("trap-exit", &["on"], "Set the current process's trap_exit flag (Erlang process_flag(trap_exit, …)); returns the previous value. When on, a linked peer's death arrives as a trappable [:EXIT pid reason] message instead of killing this process."),
    ("process-flag", &["flag", "&optional", "value"], "Read or set a per-process runtime flag on the current process (Erlang process_flag/2); returns the previous (or, with no value, current) setting. Flags: :max-heap — this process's heap limit in bytes (BEAM max_heap_size analogue; positive int sets, nil clears, absent reads). Checked after each GC against the live footprint; exceeding it raises a catchable E0045 error in this process only — uncaught, it kills just the offender (the global BROOD_MEM_LIMIT hard cap aborts the whole runtime). Set it first thing in a spawned fn to cap that process: (spawn (fn () (process-flag :max-heap 8000000) (work))). :send-errors — when truthy, a (send …) whose target NODE is unknown/disconnected raises a catchable E0060 noconnection error instead of silently dropping the message (Erlang's default; process liveness stays silent either way) — so a sender can queue-and-retry across a net-split; pairs with the net/reconnect reconnector."),
    ("spawn-count", &[], "How many green processes have been spawned since program start."),
    ("sched-stats", &[], "A snapshot map of the scheduler's cumulative counters: {:spawned :exited :preempts :steals :migrations :workers :peak-threads}. :spawned - :exited is the live-process figure; :preempts counts reduction-budget quantum exhaustions; :steals/:migrations count work-stealing activity. The scheduler half of the observability timing tier (pairs with gc-stats' :pause-* keys)."),
    ("profile-start", &["&optional", "hz"], "Arm the sampling CPU profiler at hz samples/sec (default 99, clamped 1..10000), resetting the histogram. Sampling walks each process's reified call stack (named frames) at its next VM frame boundary after every tick — no signals, near-zero cost when off (one relaxed load per frame boundary). A JIT-resident loop is attributed when it yields at its reduction-budget preempt (~once a quantum); the legacy tree-walker isn't sampled. Stop and read with (profile-stop)."),
    ("profile-stop", &[], "Disarm the sampling profiler and return the histogram: a list of {:stack (fn-names... innermost-first) :count n} maps, most-sampled first. Empty list if never armed. A sample whose frames were all anonymous appears with :stack (\"<anonymous>\")."),
    ("system-monitor", &["&optional", "pid", "opts"], "Read, arm, or clear the kernel system monitor — runtime events pushed to ONE subscriber process as [:system kind subject-pid detail] mailbox messages (Erlang system_monitor/2 shape; the observability event stream's kernel sources). Kinds: :gc {:pause-us :collections :live} (a collection of subject's heap finished), :spawn (detail = parent pid), :exit (detail = the structured exit reason monitors see), :deopt (detail = the JIT arm's fn name, or nil). No args reads the current config map (nil if unarmed); (system-monitor nil) clears; (system-monitor pid) arms every event at pid; (system-monitor pid {:gc true :gc-min-pause-us 1000 :exit true}) selects exactly the truthy keys (:gc-min-pause-us = report only pauses that long, BEAM's long_gc). Arming/clearing returns the PREVIOUS config. One subscriber at a time (last wins); events about the subscriber itself are never sent (no feedback loops), and the subscriber's death disarms the stream. Policy lives in telemetry/watch-runtime, which re-emits these as telemetry events."),
    ("peak-threads", &[], "High-water mark of OS threads running processes concurrently."),
    ("worker-threads", &[], "The size of the scheduler's worker-thread pool (about nproc)."),
    ("features", &[], "The optional build features this runtime was compiled with, as a vector of keywords (e.g. [:jit :treesit :gui]). A *bound* builtin does not imply a working one — with the `gui` feature off, `gui-open` is still bound and raises at call time — so an app that degrades rather than fails must ask the build, not `bound?`. `feature?` is the predicate over this."),
    ("build-id", &[], "This brood build's identity as \"<version>+<git-sha>+<binary-stamp>\" (e.g. \"0.1.0+dcab7ca+18f2e1a9b3c4d5e6\") — the correct staleness stamp for an on-disk cache of anything the kernel computes. Changes on any rebuild, committed or not: the binary-stamp half is this executable's own mtime, read at runtime, so it can't go stale the way a git-sha-only stamp would across an uncommitted local rebuild."),
    ("steal-count", &[], "How many fresh processes the scheduler work-stole across worker threads since program start; 0 means placement-at-spawn kept the pool even."),
    ("register", &["name", "pid"], "Bind a local name so peers can address this process via {:name name :node this-node}. Returns the pid."),
    ("whereis", &["name"], "The local pid registered under `name`, or nil. Strictly local — does not query other nodes."),
    ("node-name", &[], "This runtime's node name (:nonode until node-start)."),
    ("nodes", &[], "A list of currently connected peer node names."),
    ("monitor-node", &["name"], "Get [:nodedown name] when the link to node `name` goes down (heartbeat timeout or close)."),
    ("disconnect", &["name"], "Tear down the link to peer node `name` now, without exiting this process (Erlang's disconnect_node) — fires [:nodedown name] on both sides and prunes `name` from (nodes). Returns true if a link existed, false otherwise. Use it to leave a node/cluster cleanly while staying alive."),
    // ---- raw bytes (builtins/bytes.rs) ----
    ("bytes", &["&", "byte-ints"], "Build a bytes value from byte integers 0–255: (bytes 1 2 3), or (bytes [1 2 3]) / (bytes (list …)) taking a single vector/list as the sequence. An existing bytes value passes through unchanged."),
    ("byte-length", &["b"], "The number of bytes in b. O(1)."),
    ("byte-at", &["b", "i"], "The byte at index i of b as an int 0–255; errors if i is out of range."),
    ("subbytes", &["b", "start", "&optional", "end"], "The byte slice [start, end) of b as a fresh bytes value (end defaults to the length). Errors if the range is out of bounds."),
    ("bytes-concat", &["&", "iolists"], "One bytes value joining all arguments, each an iolist (ADR-139): a string (UTF-8), a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those. The in-memory materialiser of the iolist model."),
    ("bytes-index-of", &["haystack", "needle", "&optional", "from"], "The first index of the needle bytes within haystack at or after from (default 0), or -1 if absent. The byte-protocol workhorse (locate a \\r\\n\\r\\n, a frame delimiter, …)."),
    ("bytes->list", &["b"], "The bytes b as a list of integers 0–255."),
    // ---- numeric (builtins/numeric.rs) ----
    ("max", &["x", "&", "more"], "The greatest of one or more numbers (int/float/decimal), compared numerically; the result keeps its own type."),
    ("min", &["x", "&", "more"], "The least of one or more numbers (int/float/decimal), compared numerically; the result keeps its own type."),
    // ---- namespace / sequence-view / distribution ----
    ("current-ns", &[], "The current compilation namespace as a symbol, or nil at the root namespace (top level)."),
    ("seqview?", &["x"], "True if x is a lazy sequence view — the reducible produced by range/map/filter/… before it is realized (into/count/…)."),
    ("demonitor-node", &["name"], "Cancel this process's node monitor for node `name` (undo monitor-node); a no-op if none is registered. Returns nil."),
];

/// The `(params, doc)` for a primitive `name`, or `(&[], "")` if undocumented.
fn primitive_doc(name: &str) -> (&'static [&'static str], &'static str) {
    PRIMITIVE_DOCS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|&(_, p, d)| (p, d))
        .unwrap_or((&[], ""))
}

#[cfg(test)]
mod primitive_docs_tests {
    use super::*;
    use crate::core::heap::Heap;
    use std::collections::HashSet;

    // Register every primitive into a fresh LOCAL env (not the global one, so
    // `env_chain_names` can enumerate it) and return the names bound to a native.
    fn registered_primitive_names() -> Vec<String> {
        let mut heap = Heap::new();
        let root = heap.new_env(None);
        register(&mut heap, root);
        heap.env_chain_names(root)
            .into_iter()
            .filter(|&sym| matches!(heap.env_get(root, sym), Some(Value::Native(_))))
            .map(value::symbol_name)
            .collect()
    }

    // Drift guard: `register()` and `PRIMITIVE_DOCS` live ~2000 lines apart and
    // agree only by string key, so a new primitive (or a rename/removal) can
    // silently lose its doc. This pins the contract:
    //   1. every USER-FACING primitive (a non-`%` native) has a PRIMITIVE_DOCS
    //      entry — `%`-prefixed ops are internal kernel primitives (wrapped by a
    //      prelude fn/macro; never called directly), so they're exempt;
    //   2. no PRIMITIVE_DOCS entry is an orphan (names a primitive that no longer
    //      registers).
    // A new public primitive without a doc, or a doc left behind by a rename,
    // fails here — the doc is what `(doc …)`, the LSP, and completion surface.
    #[test]
    fn every_user_facing_primitive_is_documented_and_no_orphan_docs() {
        let registered: Vec<String> = registered_primitive_names();
        let doc_names: HashSet<&str> = PRIMITIVE_DOCS.iter().map(|&(n, _, _)| n).collect();
        let reg_names: HashSet<&str> = registered.iter().map(|s| s.as_str()).collect();

        let mut undocumented: Vec<&str> = registered
            .iter()
            .map(|s| s.as_str())
            .filter(|n| !n.starts_with('%') && !doc_names.contains(n))
            .collect();
        undocumented.sort();

        let mut orphan: Vec<&str> = PRIMITIVE_DOCS
            .iter()
            .map(|&(n, _, _)| n)
            .filter(|n| !reg_names.contains(n))
            .collect();
        orphan.sort();

        assert!(
            undocumented.is_empty(),
            "user-facing primitives missing a PRIMITIVE_DOCS entry (add one, or prefix with `%` if truly internal): {undocumented:?}"
        );
        assert!(
            orphan.is_empty(),
            "PRIMITIVE_DOCS entries with no registered primitive (stale after a rename/removal): {orphan:?}"
        );
    }
}
