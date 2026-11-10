//! The type lattice — step 1 of the set-theoretic type direction (ADR-023,
//! semantic subtyping, set-theoretic + gradual; Castagna, Duboc, Valim).
//!
//! A [`Ty`] **is a set of values**, represented as a bitset over the runtime
//! [`Tag`]s (the value-set atoms — see [`crate::core::value::Tag`]). On this model the
//! set operations *are* the type operations:
//!
//! - union (`∪`)        — "could be either"        → bitwise OR
//! - intersection (`∩`) — "both at once"           → bitwise AND
//! - negation (`¬`)     — "everything except"      → complement within the universe
//! - subtyping (`⊆`)    — **semantic subtyping**: `a` is a subtype of `b` iff the
//!   set `a` is contained in the set `b`. No syntactic rules — inclusion is the
//!   definition. [`Ty::NEVER`] (`⊥`, the empty set) is a subtype of everything;
//!   everything is a subtype of [`Ty::ANY`] (`⊤`, all tags).
//!
//! This is a *minimal* set-theoretic lattice: the atoms are the 12 flat tags, so
//! it can express "int | string" or "not nil" but not yet *structured* types
//! (function arrows, a vector's element type) or the gradual `dynamic()` type.
//! Both are later steps; nothing in the language consumes `Ty` yet. This module
//! is just the algebra, with its own tests.
//!
//! `check` (the advisory type checker — the lattice's first consumer) lives
//! alongside it here.

// Without the `checker` feature (a game in a tab) the checker's entry points are not
// registered, and what the runtime still reaches — the signature index lazy loading reads
// (ADR-370), the introspection that prints a sig — is all that is linked.
#[cfg_attr(not(feature = "checker"), allow(dead_code))]
pub mod check;

mod construct;
mod gradual;
mod literal_set;
mod merge;
mod range;
mod recursive;
mod term_ops;
mod term_relations;
mod union_of_terms;

pub use gradual::{GradualTy, set_strict_checking, strict_checking};
pub(crate) use literal_set::LitSet;
pub use range::Range;
#[cfg(test)]
pub(crate) use recursive::{rec_budget_exhaustions, rec_unrolls_spent};

use literal_set::*;
use merge::*;
use recursive::*;
use term_relations::*;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::core::value::{self, Symbol, Tag, Value};

/// Every tag, in bit order — for iterating a `Ty`'s members (printing, etc.) and
/// the source of [`TAG_COUNT`]. It is [`Tag::ALL`], whose order is checked at compile
/// time beside it; `tag_universe_is_consistent` (below) guards the universe size.
const ALL_TAGS: [Tag; Tag::ALL.len()] = Tag::ALL;

/// The number of tag atoms — derived from [`ALL_TAGS`], not hand-counted.
const TAG_COUNT: u32 = ALL_TAGS.len() as u32;
/// `Ty` is a `u32`, so at most 32 atoms fit. The `UNIVERSE` mask
/// `(1u32 << TAG_COUNT) - 1` would otherwise fail const-eval with a cryptic
/// shift-overflow message when someone added the 33rd atom — this surfaces
/// the cap with a clear message right where the lattice width is set. Widen
/// `Ty(u32)` to `Ty(u64)` (and this assert) to lift the cap.
const _: () = assert!(
    TAG_COUNT <= 32,
    "Ty is u32-wide; widen the type to add more than 32 atoms",
);
/// All bits set for the atoms — the universe `⊤`. Follows [`TAG_COUNT`].
/// Computed in `u64` then narrowed: at the cap (`TAG_COUNT == 32`) the direct
/// `1u32 << 32` would overflow in const-eval, so the wider shift sidesteps it
/// (`(1u64 << 32) - 1 == 0xFFFF_FFFF`, which narrows to `u32::MAX` losslessly).
const UNIVERSE: u32 = ((1u64 << TAG_COUNT) - 1) as u32;

/// The bit position of `tag` in a [`Ty`]'s bitset — its `#[repr(u8)]`
/// discriminant. No hand-maintained mapping (so no collisions possible); the
/// declaration order of [`Tag`] is the bit order.
const fn bit(tag: Tag) -> u32 {
    tag as u8 as u32
}

/// The function tags — the members a function-arrow refinement applies to. A
/// closure is [`Tag::Fn`], a Rust builtin is [`Tag::Native`]; a function *type*
/// `(int) -> int` describes both.
const FN_BITS: u32 = (1u32 << bit(Tag::Fn)) | (1u32 << bit(Tag::Native));

/// The sequence tags an element-type refinement applies to — a list (`pair`;
/// `nil` is the empty list, no elements) or a `vector`.
const SEQ_BITS: u32 =
    (1u32 << bit(Tag::Pair)) | (1u32 << bit(Tag::Vector)) | (1u32 << bit(Tag::Set));

/// The map tag — the one tag a key/value refinement applies to.
const MAP_BIT: u32 = 1u32 << bit(Tag::Map);
/// The record tag (ADR-542) — a `defrecord` value: its own kind, never a map.
const RECORD_BIT: u32 = 1u32 << bit(Tag::Record);
/// The tags a record SHAPE (`fields`) refines: a map's keyword-keyed shape, and a record's
/// fields. A shape carrying the `__id__` identity literal describes a record and is built on
/// `RECORD_BIT`; any other is a plain map's ([`Ty::record_shape`]).
const SHAPE_BITS: u32 = MAP_BIT | RECORD_BIT;
/// The nil tag — the one non-map a keyword read does not raise on (it answers `nil`).
const NIL_BIT: u32 = 1u32 << bit(Tag::Nil);

/// The bytes tag. Not refinement-bearing — a `bytes` is a sequence of octets, so its
/// element type is fixed by the kind rather than carried — but [`Ty::elem_ty`] names it
/// to answer for that fixed element type.
const BYTES_BIT: u32 = 1u32 << bit(Tag::Bytes);

/// The vector tag alone (not `pair` too) — the one tag a *positional* tuple
/// refinement applies to. Deliberately narrower than `SEQ_BITS`: a tuple is a
/// fixed-arity, per-position-typed shape of a `[ ]` vector (ADR-003 already keeps
/// vectors and cons-list `pair`s separate). A list built by `(list a b)` or read from
/// a quoted literal has its own positional shape on the `pair` tag alone —
/// `list_shape`, see [`PAIR_BIT`].
const VECTOR_BIT: u32 = 1u32 << bit(Tag::Vector);

/// The pair tag alone — the one tag a *positional* LIST shape refines.
/// `(list m '(…))` is a two-element list whose first is a map and whose second a pair,
/// and `(first …)` of it is the map, not `pair | map`: the element union threw the
/// positions away exactly as it did for vectors before ADR-128. `nil` — the empty list
/// — is not a `pair`, so a shape describes a non-empty list of exactly its arity.
const PAIR_BIT: u32 = 1u32 << bit(Tag::Pair);

/// The keyword tag — the one tag a literal (singleton) refinement applies to. A
/// keyword-literal type `:maximized` refines the keyword members to exactly the
/// listed keyword symbols (set-theoretic literal types, ADR; keyword-only first).
const KEYWORD_BIT: u32 = 1u32 << bit(Tag::Keyword);

/// The int tag — a second, independent literal-bearing tag (ADR-117), the same
/// pattern `KEYWORD_BIT` established: `5` in type position refines the int
/// members to exactly the listed integers. Independent of `KEYWORD_BIT` (a
/// different bit), so `(or :ok 5)` carries both refinements at once with no
/// special-casing — see `docs/type-int-literals.md`.
const INT_BIT: u32 = 1u32 << bit(Tag::Int);

/// A third and fourth independent literal-bearing tag (ADR-120), same pattern
/// as `KEYWORD_BIT`/`INT_BIT`.
const BOOL_BIT: u32 = 1u32 << bit(Tag::Bool);
const STR_BIT: u32 = 1u32 << bit(Tag::Str);

/// Max refinement-tree node count an inferred `Ty` retains; a `Ty` built past it (by
/// `union` or a structural constructor) drops its structural refinements to "any". Bounds
/// the SIZE of an inferred type — so a recursive value-builder can't grow a type whose
/// `==`/`Hash`/`is_subtype` (recursive over `Arc` refinements, walking a shared DAG as a
/// tree) goes superlinear (KI-13). It applies to a DECLARED shape too, and 64 was where
/// bedit's `model` deftype sat on 2026-09-16: one more optional field — a flat `map` —
/// widened the whole record to a bare `map`, silently, and every `model`-typed sig in the
/// project read `number` where it had read `int`. The fixpoints converge by
/// `widened_below`, not by this cap, so it is the safety net alone; four times the room
/// keeps a written shape whole, and `deftype` now says so when one is still over it.
pub const MAX_TY_NODES: usize = 256;

/// Max **terms** an inferred union keeps before collapsing to one widened term (see
/// [`Ty::alts`]). Four covers the shapes that occur — a tagged union of two or three
/// record/tuple alternatives, optionally with `nil` — while keeping every set
/// operation's pairwise work trivially bounded. It is NOT a knob: the relations quantify
/// over subsets of the alternatives (ADR-289), and eight took the checker on the match
/// compiler's own file from 0.6 s to past five minutes. A result with more arms than
/// this (hatch's `oidc/complete`, five `[:error …]` beside its `[:ok …]`) keeps its tag
/// because the overflow collapse in `from_terms` merges same-tag shapes first.
const MAX_TY_TERMS: usize = 4;
/// The widest int interval read as its enumeration against a literal set
/// (`Ty::enumerated_int_range`): `(int 1 2) ⊆ 1 | 2` needs the two values listed. A
/// literal set this checker meets is a `match`'s handful of arms, so a wider interval
/// is never inside one and enumerating it would only spend the allocation.
const MAX_ENUMERATED_RANGE: i64 = 64;
/// How many terms one type may subtract (ADR-288). Same bounded-size discipline as
/// [`MAX_TY_TERMS`]: beyond this the extra subtractions are dropped, which *widens* the
/// type — the safe direction, since a wider type warns less rather than wrongly.
const MAX_NEG_TERMS: usize = 4;

/// A record shape — the `fields` refinement's payload (ADR-264).
///
/// A shape is **not** just a field map: it also says what every key it does *not*
/// declare holds. That single addition is what makes a record type say what a value
/// *is not*, which is what a tagged union needs:
///
/// - **closed** (`rest` = `nil`) — the plain `(record :a int)`. Any other key is
///   absent, and Brood reads an absent key as `nil`, so `(get r :b)` is exactly `nil`.
/// - **open** (`rest` = `any`) — `(record &open :a int)`. Other keys may be present
///   with any value, so nothing can be concluded about them.
///
/// Modelling openness as *the type of the undeclared keys* rather than as a boolean
/// is what keeps every set operation uniform: subtyping, disjointness and
/// intersection all quantify over `keys(a) ∪ keys(b)` and then compare the two
/// `rest`s, with no special case for either kind.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RecordShape {
    /// Declared fields: name → (type, required?). An *optional* field may be absent,
    /// which reads as `nil` — see [`RecordShape::field_ty`].
    fields: BTreeMap<Symbol, (Ty, bool)>,
    /// The type of every key not declared in `fields`.
    rest: Ty,
}

impl RecordShape {
    /// The type a `(get r k)` yields for `k` on a value of this shape: the declared
    /// type for a required field, that type *or `nil`* for an optional one (it may be
    /// absent), and `rest` for a key the shape does not declare.
    ///
    /// This is the single reading every relation uses, which is why a closed record's
    /// undeclared key (`nil`) and an open record's (`any`) need no special-casing.
    fn field_ty(&self, key: Symbol) -> Ty {
        match self.fields.get(&key) {
            Some((ty, true)) => ty.clone(),
            Some((ty, false)) => ty.clone().union(Ty::of(Tag::Nil)),
            None => self.rest.clone(),
        }
    }

    /// The type `(get r k default)` yields: the declared type for a required field (the
    /// default is never consulted), the declared type *or the default* for an optional one
    /// (present → declared, absent → default), the default alone for a key a CLOSED shape
    /// does not declare (always absent), and `rest ∪ default` on an open shape. The
    /// absence-`nil` of [`field_ty`](Self::field_ty) is replaced by the default, and a
    /// declared type that itself admits `nil` keeps it — a stored `nil` is present, and
    /// `get` returns it, not the default.
    fn field_ty_with_default(&self, key: Symbol, default: Ty) -> Ty {
        match self.fields.get(&key) {
            Some((ty, true)) => ty.clone(),
            Some((ty, false)) => ty.clone().union(default),
            None if self.is_open() => self.rest.clone().union(default),
            None => default,
        }
    }

    /// Is this shape open — may a value carry keys it does not declare?
    fn is_open(&self) -> bool {
        !self.rest.is_never() && !self.rest.is_subtype(&Ty::of(Tag::Nil))
    }

    /// Every key either shape declares — the domain both must be compared over.
    fn keys_with(&self, other: &RecordShape) -> BTreeSet<Symbol> {
        self.fields
            .keys()
            .chain(other.fields.keys())
            .copied()
            .collect()
    }
}

/// The tags whose members have a LENGTH — what `count` measures: a list (`pair`; the
/// empty list is `nil`, always 0, and carries none), a vector, a set, a map, a string, a
/// `bytes`. The `len` slot describes every such member of a term at once.
const COUNT_BITS: u32 = SEQ_BITS | MAP_BIT | STR_BIT | BYTES_BIT;

/// A set-theoretic type — a **set of runtime [`Tag`]s** with optional
/// *structured refinements* on its function and sequence members (Step 5+,
/// ADR-078).
///
/// The flat `tags` bitset is the coarse set and carries the whole pre-Step-5
/// behaviour verbatim. Two refinements layer on top, each `None` by default
/// ("any"):
/// - `arrow` refines the function members (`Fn`/`Native`) to those matching a
///   specific signature — `(int) -> int` is `{tags: Fn|Native, arrow: Some(…)}`.
///   Reused from [`Sig`] (an arrow type *is* a signature).
/// - `elem` refines the sequence members (`pair`/`vector`) to those whose
///   elements have a given type — `vector<int>` is `{tags: Vector, elem: Some(int)}`.
///
/// **Advisory-soundness rule:** the set operations may only ever *widen* a
/// refinement (toward `None` = "any") when they can't represent the exact
/// result. Widening over-approximates the set, so it can only ever suppress a
/// warning — never manufacture a false one. [`is_disjoint`](Ty::is_disjoint) is
/// decided on tags alone and never inspects a refinement; the precise arrow check
/// (callback compatibility) is a dedicated step in [`check`].
///
/// No longer `Copy` (the `Arc` refinements) but cheap to `Clone` — a `u32` plus
/// refcount bumps. The flat case is two null pointers.
#[derive(Clone, Debug)]
pub struct Ty {
    /// The set of possible runtime tags — always present; the coarse set.
    tags: u32,
    /// Refinement of the function members (`Fn`/`Native`), when statically known.
    /// `None` means "any function" (the permissive default).
    arrow: Option<Arc<Sig>>,
    /// Refinement of the function members to a **set of alternative
    /// signatures** — an overload, e.g. `(int -> int) and (bool -> bool)`.
    /// Only ever holds 2+ *distinct* `Sig`s; a single one always collapses
    /// back to `arrow` (so every existing single-arrow consumer is untouched
    /// for the common case). `None` alongside `arrow: None` means "any
    /// function"; `None` alongside `arrow: Some(_)` means "exactly one known
    /// signature" — see `docs/type-arrow-intersection.md`.
    overload: Option<Arc<Vec<Sig>>>,
    /// Refinement of the sequence members (`pair`/`vector`) — the element type,
    /// when statically known. `None` means "elements of any type".
    elem: Option<Arc<Ty>>,
    /// Refinement of the map member (`map`) — `(key-type, val-type)`, when
    /// statically known.  `None` means "keys and values of any type".
    map_kv: Option<Arc<(Ty, Ty)>>,
    /// Refinement of the map member (`map`) to a heterogeneous record shape —
    /// `field name → (declared type, required?)`, when statically known from a
    /// `(record …)` annotation. `None` means "no declared shape". Mutually
    /// exclusive with `map_kv` in practice (a `Ty` is built by either
    /// `map_of` or `record_of`), but the two refinements are independent
    /// fields so the generic union/intersect machinery treats them exactly
    /// like every other refinement pair — see `docs/type-records.md`.
    fields: Option<Arc<RecordShape>>,
    /// Refinement of the vector member (`vector`) to a **positional** shape —
    /// one type per index, when statically known from a `(tuple …)`
    /// annotation (ADR-128). `None` means "no declared positional shape".
    /// Mutually exclusive with `elem` in practice (a `Ty` is built by either
    /// `vector_of` or `tuple_of`), same independent-fields story as
    /// `map_kv`/`fields` above — see `docs/type-tuples.md`.
    tuple: Option<Arc<Vec<Ty>>>,
    /// Refinement of the pair member (`pair`) to a **positional** shape — one type per
    /// index, a non-empty list of exactly that many elements: `(list a b)`, `'(1 2 3)`,
    /// `(cons x <shaped>)`. The list sibling of `tuple`, same independent-field story;
    /// `None` means "no known positional shape". See [`PAIR_BIT`].
    list_shape: Option<Arc<Vec<Ty>>>,
    /// Refinement of the keyword member (`keyword`) to a literal set — the exact
    /// keyword symbols admitted, e.g. `{:maximized, :fullboth}`. `None` means "any
    /// keyword". When `Some`, the `Keyword` bit is in `tags` and the set is
    /// non-empty; the keyword member is constrained to the set while every *other*
    /// tag in `tags` stays open (so `(or :a :b nil)` admits the two keywords *and*
    /// `nil`). Unlike the other refinements, union of two literal sets is *exact*
    /// (the set-union), not a widening — so `(or :a :b)` keeps both.
    lit: Option<Arc<LitSet<Symbol>>>,
    /// Refinement of the int member (`int`) to a literal set — the exact
    /// integers admitted, e.g. `{5, 6}` (ADR-117). Independent of `lit`
    /// (a different tag, `INT_BIT` not `KEYWORD_BIT`), so both can be `Some`
    /// at once (`(or :ok 5)`). Same semantics as `lit` throughout: union is
    /// exact, not a widening; every other tag stays open. `BigInt`-range
    /// literals aren't representable here — see `docs/type-int-literals.md`.
    lit_int: Option<Arc<LitSet<i64>>>,
    /// Refinement of the bool member (`bool`) to a literal set (ADR-120) —
    /// `{true}`, `{false}`, or (equivalent to unrefined) `{true, false}`.
    /// Independent tag/field, same semantics as `lit`/`lit_int` throughout.
    lit_bool: Option<Arc<LitSet<bool>>>,
    /// Refinement of the string member (`string`) to a literal set (ADR-120).
    /// Stores owned `String` content, not a heap `StrId` — two textually
    /// identical string literals can have different underlying heap handles,
    /// so comparing/ordering by content (not handle identity) is what makes
    /// set operations correct. Independent tag/field, same semantics as
    /// `lit`/`lit_int` throughout.
    lit_str: Option<Arc<LitSet<String>>>,
    /// **Alternative terms** — the disjunctive tail of a union this one term cannot
    /// hold exactly (ADR-262). `None` for the overwhelmingly common single-term type,
    /// which behaves exactly as it always did.
    ///
    /// A `Ty` is a *set of values*, and a set of values is a union of terms. One term
    /// carries one refinement per slot, so `(or (tuple int) (tuple string))` had no
    /// representation at all: `union` widened both away and the type became bare
    /// `vector`. That is sound, and it made the tagged-union idiom — `{:ok v}` or
    /// `{:error e}`, the shape most Brood code returns — invisible to every check.
    ///
    /// So a union that cannot merge exactly keeps both terms. The invariants, all
    /// maintained by [`Ty::from_terms`]:
    /// - every alternative is itself alts-free (the representation is two levels, not
    ///   a tree), and non-`never`;
    /// - no term is a subtype of another (absorbed on construction);
    /// - at most [`MAX_TY_TERMS`] terms — beyond that they collapse by the old
    ///   widening merge, so the size of a `Ty` stays bounded (the KI-13 property).
    ///
    /// Every *refinement accessor* (`as_arrow`, `elem_ty`, `record_fields`, …) reports
    /// only for a single-term type and `None` for a multi-term one — exactly what a
    /// widened type reported before — so no consumer reads a refinement that holds for
    /// only part of the union. The set relations (`is_subtype`, `is_disjoint`,
    /// `intersect`, `negate`) are the ones that got sharper.
    alts: Option<Arc<Vec<Ty>>>,
    /// **Subtracted terms** — this term denotes its positive part *minus* the union of
    /// these (ADR-288). `None` for the overwhelmingly common type that subtracts nothing.
    ///
    /// This is what makes a **structural** complement exact. Tags and literal sets can be
    /// complemented in place (flip the bits, flip `In`/`Out`), but "a vector whose element
    /// type is not `int`" is not a shape the positive slots can hold, so `¬(vector int)`
    /// widened all the way to `any` — and `(vector int) ∩ ¬(vector int)` came out
    /// `(vector int)` rather than `never`, which is a guard's else-branch learning nothing.
    ///
    /// Every subtracted term is itself positive (no `alts`, no `neg` of its own), so the
    /// representation stays two levels rather than a tree. Everything else follows from
    /// one identity — `P ∖ N` is empty exactly when `P ⊆ ⋃N`, which is
    /// [`term_is_subtype_of_union`], the same procedure cross-term subtyping already uses.
    neg: Option<Arc<Vec<Ty>>>,
    /// Refinement of the int member to an INTERVAL (ADR-350): `int[0..]`, what a counter
    /// that starts at zero and only grows is. Independent of `lit_int`, which is a finite
    /// SET; when that set is positive the interval is redundant and is not stored (the
    /// canonical form), so the int member is `lit_int ∩ int_range` with either absent
    /// meaning "no constraint from that side". Unions take the hull (a widening, unlike
    /// the literal set's exact union); intersections meet; an empty meet drops the tag.
    int_range: Option<Range>,
    /// Refinement of the countable members ([`COUNT_BITS`]) to a LENGTH interval
    /// (ADR-350): `vector<int>[3]`, `list<string>[1..]`. `nil` is never in it (the empty
    /// list is its own tag, of length 0), and a `pair` is at least 1 whether or not this
    /// says so. What `(count xs)` reads, what `(= (count xs) 3)` and `(empty? xs)` narrow,
    /// and what `(nth xs 1)` consults to drop its `nil` arm.
    len: Option<Range>,
    /// **The self-reference of a recursive type** (ADR-349). A term with this
    /// set stands for the whole `Ty` whose [`mu`](Ty::mu) is set — `X` in `μX. nil |
    /// vector<X>`. Its tags are `UNIVERSE`, so a reader that does not resolve it sees
    /// `any` (sound); the set relations treat it as an UNKNOWN set: nothing but `any`
    /// contains it, it contains nothing but another self-reference, and it is never
    /// disjoint from anything. One level only: a reference names the nearest binder, and a
    /// nested `mu` body is closed (its references are its own).
    rec_ref: bool,
    /// **μ binder**: this `Ty` is a recursive type, and every `rec_ref` term in its
    /// refinement tree (not inside another binder) is this `Ty` itself. A property of the
    /// whole type, never of one term — `term_eq` ignores it, `PartialEq` compares it.
    /// [`Ty::unroll`] substitutes the type for its references one level; every set
    /// relation unrolls a μ before descending, under a coinductive assumption so the
    /// descent terminates. See `docs/type-recursive.md`.
    mu: bool,
}

/// Equality is **set** equality, not field equality: a union's terms have no
/// meaningful order (`A ∪ B` and `B ∪ A` are the same set), so a derived `PartialEq`
/// would call them different and break every memo and cache keyed on a type.
impl PartialEq for Ty {
    fn eq(&self, other: &Ty) -> bool {
        if self.mu != other.mu {
            return false;
        }
        if self.alts.is_none() && other.alts.is_none() {
            return self.term_eq(other);
        }
        let (a, b) = (self.terms_vec(), other.terms_vec());
        a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| x.term_eq(y)))
    }
}
impl Eq for Ty {}

/// Hashing must agree with that equality, so it combines the terms' hashes with an
/// order-independent XOR (the terms are distinct by construction).
impl std::hash::Hash for Ty {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut combined: u64 = 0;
        for term in self.terms_vec() {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            term.hash_term(&mut h);
            combined ^= std::hash::Hasher::finish(&h);
        }
        state.write_u64(combined);
        state.write_u8(u8::from(self.mu));
    }
}

impl Ty {
    /// Field-by-field equality of one term, ignoring `alts`.
    ///
    /// Destructured rather than field-accessed **on purpose**: a new refinement slot
    /// then fails to compile here until it is listed, instead of silently making two
    /// different types compare equal. (`#[derive(PartialEq)]` can't be used — a union's
    /// terms have no meaningful order — so this is the enforcement that replaces it.)
    fn term_eq(&self, other: &Ty) -> bool {
        let Ty {
            tags,
            arrow,
            overload,
            elem,
            map_kv,
            fields,
            tuple,
            list_shape,
            lit,
            lit_int,
            lit_bool,
            lit_str,
            alts: _,
            neg,
            rec_ref,
            mu,
            int_range,
            len,
        } = self;
        *tags == other.tags
            && *arrow == other.arrow
            && *overload == other.overload
            && *elem == other.elem
            && *map_kv == other.map_kv
            && *fields == other.fields
            && *tuple == other.tuple
            && *list_shape == other.list_shape
            && *lit == other.lit
            && *lit_int == other.lit_int
            && *lit_bool == other.lit_bool
            && *lit_str == other.lit_str
            && *neg == other.neg
            && *rec_ref == other.rec_ref
            && *mu == other.mu
            && *int_range == other.int_range
            && *len == other.len
    }

    /// Hash one term's fields, ignoring `alts` — the counterpart of [`Ty::term_eq`],
    /// destructured for the same compile-time reason.
    fn hash_term<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        let Ty {
            tags,
            arrow,
            overload,
            elem,
            map_kv,
            fields,
            tuple,
            list_shape,
            lit,
            lit_int,
            lit_bool,
            lit_str,
            alts: _,
            neg,
            rec_ref,
            mu,
            int_range,
            len,
        } = self;
        tags.hash(state);
        arrow.hash(state);
        overload.hash(state);
        elem.hash(state);
        map_kv.hash(state);
        fields.hash(state);
        tuple.hash(state);
        list_shape.hash(state);
        lit.hash(state);
        lit_int.hash(state);
        lit_bool.hash(state);
        lit_str.hash(state);
        neg.hash(state);
        rec_ref.hash(state);
        mu.hash(state);
        int_range.hash(state);
        len.hash(state);
    }
}

impl Ty {
    /// The terms of a *multi*-term type, or `None` for a single term — the display
    /// hook, kept beside [`Ty::single`] so the two read as one decision.
    pub(crate) fn alt_terms(&self) -> Option<Vec<Ty>> {
        self.alts.is_some().then(|| self.terms_vec())
    }

    /// The int interval's rendering, `[0..]`, for the renderer; empty without one.
    pub(crate) fn int_suffix(&self) -> String {
        self.int_range
            .map(display::range_suffix)
            .unwrap_or_default()
    }

    /// The length's rendering, `[3]` / `[1..]`, for the renderer; empty without one.
    pub(crate) fn len_suffix(&self) -> String {
        self.len.map(display::range_suffix).unwrap_or_default()
    }

    /// The suffix for one tag of a term: its int interval for `int`, its length for a
    /// countable tag, nothing otherwise.
    pub(crate) fn range_suffix_for(&self, tag: Tag) -> String {
        let bit_of = 1u32 << bit(tag);
        if bit_of & INT_BIT != 0 {
            self.int_suffix()
        } else if bit_of & COUNT_BITS != 0 {
            self.len_suffix()
        } else {
            String::new()
        }
    }

    /// Is this exactly a ranged int — one term, the int tag alone, nothing else refined?
    pub(crate) fn is_only_ranged_int(&self) -> bool {
        self.single().is_some_and(|t| {
            t.tags == INT_BIT && t.int_range.is_some() && t.lit_int.is_none() && t.neg.is_none()
        })
    }

    /// A single term's length and the term without it, for `to_source`.
    pub(crate) fn len_for_source(&self) -> Option<(Range, Ty)> {
        let term = self.single()?;
        let r = term.len?;
        let mut without = term.clone();
        without.len = None;
        Some((r, without))
    }

    /// A recursive type's body — the same terms, without the binder — for the
    /// renderer, which prints the binder itself.
    pub(crate) fn body_for_display(&self) -> Ty {
        Ty {
            mu: false,
            ..self.clone()
        }
    }

    /// This type as a lone term, or `None` when it is a union of several. Every
    /// *refinement* accessor goes through here: a refinement that holds for one term
    /// of a union does not hold for the union, and reporting it would be exactly the
    /// unsound reading the old widening avoided by throwing the refinement away.
    fn single(&self) -> Option<&Ty> {
        self.alts.is_none().then_some(self)
    }

    /// One term's slots in canonical form, so that one set has one representation — and a
    /// member no value can inhabit is not kept as if one could:
    ///
    /// - an **element type beside a positional shape** constrains the same member, so it is
    ///   folded into the positions (`(tuple int)` with `elem: number` is `(tuple int)`;
    ///   with `elem: string` it is no vector at all), and dropped once the shaped member is
    ///   the only sequence member it could speak for;
    /// - a **positional shape no vector can have** — an empty position, or an arity the
    ///   length slot excludes — drops its member. The length used to be dropped instead,
    ///   silently, which made `((tuple int) | string) ∩ (len (vector int) 2 2)` come out
    ///   `(tuple int)` rather than `never`: not a lower bound of its operands;
    /// - a **`pair` whose elements are `never`** is no pair (a pair has a head), and a
    ///   record shape with a required `never` field is no map;
    /// - a tuple or a list shape is its arity, so a stored length beside one that is the
    ///   only countable member is redundant and goes;
    /// - a string literal set filters by the length slot, which then goes.
    fn normalise_len(mut self) -> Ty {
        if self.rec_ref || self.mu {
            return self;
        }
        if let Some(elem) = self.elem.clone() {
            for member in [VECTOR_BIT, PAIR_BIT] {
                if self.tags & member == 0 {
                    continue;
                }
                let slot = if member == VECTOR_BIT {
                    &mut self.tuple
                } else {
                    &mut self.list_shape
                };
                if let Some(shape) = slot.as_deref()
                    && !shape.iter().all(|t| t.is_subtype(&elem))
                {
                    let met: Vec<Ty> = shape
                        .iter()
                        .map(|t| t.clone().intersect(elem.as_ref().clone()))
                        .collect();
                    *slot = Some(Arc::new(met));
                }
            }
            let seq = self.tags & SEQ_BITS;
            if (seq == VECTOR_BIT && self.tuple.is_some())
                || (seq == PAIR_BIT && self.list_shape.is_some())
            {
                self.elem = None;
            }
        }
        for member in [VECTOR_BIT, PAIR_BIT] {
            let shape = if member == VECTOR_BIT {
                &self.tuple
            } else {
                &self.list_shape
            };
            let Some(positions) = shape.as_deref() else {
                continue;
            };
            let arity_excluded = self
                .len
                .is_some_and(|len| !Range::subset(Range::point(positions.len() as i64), len));
            if self.tags & member == 0 || arity_excluded || positions.iter().any(Ty::is_never) {
                self.tags &= !member;
                if member == VECTOR_BIT {
                    self.tuple = None;
                } else {
                    self.list_shape = None;
                }
            }
        }
        if self.tags & PAIR_BIT != 0 && self.elem.as_deref().is_some_and(Ty::is_never) {
            self.tags &= !PAIR_BIT;
            self.list_shape = None;
        }
        // A vector or set of `never` is the EMPTY one, so a length that excludes 0 leaves
        // no such member — `vector<never>[2..7]` is no vector at all.
        if self.elem.as_deref().is_some_and(Ty::is_never)
            && self
                .len
                .is_some_and(|len| !Range::subset(Range::point(0), len))
        {
            if self.tuple.is_none() {
                self.tags &= !VECTOR_BIT;
            }
            self.tags &= !(1u32 << bit(Tag::Set));
        }
        if self.tags & SHAPE_BITS != 0
            && self.fields.as_deref().is_some_and(|shape| {
                shape
                    .fields
                    .values()
                    .any(|(ty, required)| *required && ty.is_never())
            })
        {
            self.tags &= !SHAPE_BITS;
        }
        // A slot whose member is gone says nothing.
        if self.tags & SEQ_BITS == 0 {
            self.elem = None;
        }
        if self.tags & MAP_BIT == 0 {
            self.map_kv = None;
        }
        if self.tags & SHAPE_BITS == 0 {
            self.fields = None;
        }
        if self.tags & FN_BITS == 0 {
            self.arrow = None;
            self.overload = None;
        }
        self.len = canon_len(self.tags, self.len);
        let shaped = (self.tags & COUNT_BITS == VECTOR_BIT && self.tuple.is_some())
            || (self.tags & COUNT_BITS == PAIR_BIT && self.list_shape.is_some());
        if shaped {
            self.len = None;
        }
        // A string-literal set knows its lengths (`len_eff`), so a stored bound is either
        // redundant or a FILTER on the set: keep the members it admits and drop it, so one
        // type has one representation (`"b"` from a meet and `"b"` from a literal are `==`).
        if self.tags & COUNT_BITS == STR_BIT
            && let (Some(bound), Some(LitSet::In(set))) = (self.len, self.lit_str.as_deref())
        {
            let kept: BTreeSet<String> = set
                .iter()
                .filter(|s| Range::subset(Range::point(s.chars().count() as i64), bound))
                .cloned()
                .collect();
            if kept.is_empty() {
                // No literal fits the bound: there is no string here at all.
                self.tags &= !STR_BIT;
                self.lit_str = None;
            } else {
                self.lit_str = Some(Arc::new(LitSet::In(kept)));
            }
            self.len = None;
        }
        self
    }

    /// Every tag any term of this type admits.
    fn all_tags(&self) -> u32 {
        match &self.alts {
            None => self.tags,
            Some(rest) => rest.iter().fold(self.tags, |acc, t| acc | t.tags),
        }
    }

    /// Does this type admit a value with `tag`?
    pub fn contains_tag(&self, tag: Tag) -> bool {
        self.all_tags() & (1u32 << bit(tag)) != 0
    }

    /// Is this the empty type `⊥` (no value inhabits it)?
    pub fn is_never(&self) -> bool {
        self.terms_vec().iter().all(Ty::term_is_never)
    }

    /// Is this single term empty? No tags at all, or — the case subtractions introduce —
    /// a positive part wholly covered by what it subtracts (ADR-288).
    ///
    /// `P ∖ N = ∅ ⟺ P ⊆ ⋃N` is the whole of it, and that question is
    /// [`term_is_subtype_of_union`] — the same procedure cross-term subtyping runs, so the
    /// emptiness decision and the subtyping decision cannot disagree by construction.
    fn term_is_never(&self) -> bool {
        if self.tags == 0 {
            return true;
        }
        // A `pair` has a head, so a pair-only term whose elements are `never` has no
        // inhabitant. This is what makes `list<A> ∩ list<B>` EMPTY for disjoint `A`/`B`:
        // `list_of` is pair-only (the empty list is `nil`, a separate tag), so the old
        // reading — "the empty list is in both" — was wrong, and it hid every misuse of a
        // list-typed call result against a list parameter (the `∩ ≠ ⊥` relation could
        // never fire). `nil | list<never>` is still just `nil`, as it should be.
        if self.tags == (1u32 << bit(Tag::Pair)) && self.elem.as_deref().is_some_and(Ty::is_never) {
            return true;
        }
        match &self.neg {
            None => false,
            Some(negs) => term_is_subtype_of_union(&self.positive(), negs),
        }
    }

    /// This term's subtractions, for rendering. `None` when it subtracts nothing.
    pub(super) fn subtracted(&self) -> Option<&[Ty]> {
        self.neg.as_deref().map(|v| v.as_slice())
    }

    /// This term without its subtractions — the `P` of `P ∖ N`.
    fn positive(&self) -> Ty {
        Ty {
            neg: None,
            ..self.clone()
        }
    }

    /// Is this the universe `⊤` (every value inhabits it)?
    pub const fn is_any(&self) -> bool {
        self.tags == UNIVERSE
    }

    /// Does this type name a collection kind without saying what is in it — `any`, or a
    /// single flat term with a sequence or map tag and no element, key/value, field or
    /// tuple refinement (`nil | list`, `seqable`, `countable`, bare `map`)? These are the
    /// answers call-site specialization can sharpen: a function whose flat return is the
    /// `seqable` its parameter was demanded to be (it returns that parameter on one
    /// branch) is `list<string>` at a call that hands it one, and a recursive list builder
    /// whose flat return is `nil | list` is `nil | list<int>` at its call. A return of
    /// `int`, `string` or `list<int>` says all a re-typing could, and a union of several
    /// terms already says which.
    pub fn is_unrefined_collection(&self) -> bool {
        if self.is_any() {
            return true;
        }
        self.alts.is_none()
            && self.neg.is_none()
            && self.tags & (SEQ_BITS | MAP_BIT) != 0
            && self.arrow.is_none()
            && self.elem.is_none()
            && self.map_kv.is_none()
            && self.fields.is_none()
            && self.tuple.is_none()
            && self.list_shape.is_none()
    }
}

mod display;

mod sig;
pub use sig::Sig;

#[cfg(test)]
mod tests;
