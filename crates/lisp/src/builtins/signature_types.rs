//! The lattice shorthands every domain's `register` spells its signatures in — see
//! `types::Ty` for the algebra. NUMBER = int ∪ float, LIST = nil ∪ pair, seq = list ∪
//! vector (the receivers of first/rest). `callable` = fn ∪ native (a thunk or
//! applicable). `ANY` is the "no useful info" lane — overlaps everything, so the
//! disjointness checker never warns against it.
//!
//! `const` (not `static`) so each use re-materialises a fresh `Ty` — `Ty` is not `Copy`
//! (it carries an optional `Arc` arrow refinement, ADR-078), but a `const` mention is
//! inlined, so a signature can name these by value with no `.clone()`. Lowercase names
//! kept deliberately: they read as type shorthands, not globals.
#![allow(non_upper_case_globals)]

use crate::core::value::Tag;
use crate::types::Ty;

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
pub(crate) const any: Ty = Ty::ANY;
pub(crate) const int: Ty = Ty::of(Tag::Int);
pub(crate) const num: Ty = Ty::NUMBER;
pub(crate) const float: Ty = Ty::of(Tag::Float);
pub(crate) const string: Ty = Ty::of(Tag::Str);
pub(crate) const rope: Ty = Ty::of(Tag::Rope);
pub(crate) const socket_ty: Ty = Ty::of(Tag::Socket);
pub(crate) const subprocess_ty: Ty = Ty::of(Tag::Subprocess);
pub(crate) const table_ty: Ty = Ty::of(Tag::Table);
pub(crate) const bytes_ty: Ty = Ty::of(Tag::Bytes);
pub(crate) const decimal_ty: Ty = Ty::of(Tag::Decimal);
pub(crate) const ratio_ty: Ty = Ty::of(Tag::Ratio);
pub(crate) const kw: Ty = Ty::of(Tag::Keyword);
pub(crate) const sym: Ty = Ty::of(Tag::Sym);
pub(crate) const bool_ty: Ty = Ty::of(Tag::Bool);
pub(crate) const nil_ty: Ty = Ty::of(Tag::Nil);
pub(crate) const pair: Ty = Ty::of(Tag::Pair);
pub(crate) const vec_ty: Ty = Ty::of(Tag::Vector);
pub(crate) const map_ty: Ty = Ty::of(Tag::Map);
pub(crate) const set_ty: Ty = Ty::of(Tag::Set);
pub(crate) const pid_ty: Ty = Ty::of(Tag::Pid);
pub(crate) const ref_ty: Ty = Ty::of(Tag::Ref);
pub(crate) const list_ty: Ty = Ty::LIST;
// `bytes` is seqable too: `first`/`rest`/`nth` iterate its octets at runtime.
pub(crate) const seq: Ty = Ty::of_tags(&[Tag::Nil, Tag::Pair, Tag::Vector, Tag::Bytes]);
// What `Heap::seq_items` actually materialises — the domain of the two sort prims,
// and NOT `seq`: it takes a **set** (which `seq` omits) and rejects **bytes** (which
// `seq` admits), so borrowing `seq` here was wrong in both directions. Nothing
// noticed while `sort` carried no signature at all and its argument typed as `any`;
// declaring one made `(sort coll)` warn inside its own body. A range needs no tag —
// it materialises as a pair.
pub(crate) const seq_items_ty: Ty = Ty::of_tags(&[Tag::Nil, Tag::Pair, Tag::Vector, Tag::Set]);
// `first`/`rest` additionally walk a **set** (as its elements) and a **map** (as
// its `[k v]` pairs), matching `seq`/`map`/`fold`/`last`. Kept separate from
// `seq` so widening the head/tail pair doesn't silently widen every other
// sequence primitive's domain.
pub(crate) const seqable: Ty = Ty::of_tags(&[
    Tag::Nil,
    Tag::Pair,
    Tag::Vector,
    Tag::Bytes,
    Tag::Set,
    Tag::Map,
]);
pub(crate) const callable: Ty = Ty::of_tags(&[Tag::Fn, Tag::Native]);
// An **iolist** (ADR-139): a string, a `bytes`, a byte int 0–255, or an
// arbitrarily nested list/vector of iolists (nil = empty). The lattice can't
// express the recursion, so this is the shallow surface — the runtime
// flattener (`flatten_iolist`) enforces the leaves.
pub(crate) const iolist: Ty = Ty::of_tags(&[
    Tag::Str,
    Tag::Bytes,
    Tag::Int,
    Tag::Pair,
    Tag::Vector,
    Tag::Nil,
]);
