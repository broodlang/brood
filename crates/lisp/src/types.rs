//! The type lattice — step 1 of the set-theoretic type direction (ADR-023,
//! inspired by Elixir's set-theoretic + gradual type system).
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

pub mod check;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::core::value::{self, Symbol, Tag, Value};

/// Every tag, in bit order — for iterating a `Ty`'s members (printing, etc.) and
/// the source of [`TAG_COUNT`]. **Must list every [`Tag`] variant in discriminant
/// order**; the compiler can't enumerate variants, so `tag_universe_is_consistent`
/// (below) is what guards completeness, ordering, and the universe size.
const ALL_TAGS: [Tag; 24] = [
    Tag::Nil,
    Tag::Bool,
    Tag::Int,
    Tag::Float,
    Tag::Sym,
    Tag::Keyword,
    Tag::Str,
    Tag::Pair,
    Tag::Vector,
    Tag::Fn,
    Tag::Macro,
    Tag::Native,
    Tag::Map,
    Tag::Ref,
    Tag::Pid,
    Tag::Rope,
    Tag::Socket,
    Tag::Subprocess,
    Tag::Table,
    Tag::Bytes,
    Tag::Decimal,
    Tag::Set,
    Tag::Ratio,
    Tag::Failure,
];

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

/// The pair tag alone — the one tag a *positional* LIST shape refines (2026-09-13).
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
/// A refinement of one tag's values to a set — stated positively (**exactly these**) or
/// negatively (**anything but these**).
///
/// The negative case is what makes a literal complement representable, and therefore what
/// makes an equality test narrow its *else* branch: `(or :ok :err) ∩ ¬:ok` is `:err`, not
/// the unnarrowed union it used to be. Before this, negating a literal set widened to the
/// whole tag — `¬:ok` was `any` — so `(if (= tag :ok) …)` could only refine on the true
/// side (`then_only`, ADR-263's remaining gap).
///
/// The domain of `Out` is infinite for keywords, ints and strings, which is exactly why it
/// has to be held negatively rather than enumerated. **Bool is the exception**: its domain
/// is `{true, false}`, so a complement there is finite and is normalised back to `In`
/// where it is produced ([`Ty::negate_term`]) — no `Out(bool)` is ever constructed, and
/// the rules below may assume an `Out` set has an infinite complement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum LitSet<T: Ord> {
    /// Exactly these values.
    In(BTreeSet<T>),
    /// Every value of the tag except these. Never empty — an empty exclusion is
    /// "every value", which is the unrefined `None` slot.
    Out(BTreeSet<T>),
}

impl<T: Ord> LitSet<T> {
    /// The positively-listed members, or `None` when the set is stated negatively and
    /// cannot be enumerated. Every consumer outside this module reads literals through
    /// this, so a negative set reads exactly like an unrefined one — the conservative
    /// widening those consumers already handle.
    fn members(&self) -> Option<&BTreeSet<T>> {
        match self {
            LitSet::In(set) => Some(set),
            LitSet::Out(_) => None,
        }
    }

    /// The excluded values, or `None` for a positive set. For rendering.
    fn excluded(&self) -> Option<&BTreeSet<T>> {
        match self {
            LitSet::Out(set) => Some(set),
            LitSet::In(_) => None,
        }
    }

    /// Is this set empty — i.e. does the tag admit no value at all? Only a positive
    /// empty set is; a negative one excludes finitely many from an infinite domain.
    fn is_empty(&self) -> bool {
        matches!(self, LitSet::In(set) if set.is_empty())
    }
}

impl<T: Ord + Clone> LitSet<T> {
    /// This set's complement *within its tag*. `In ↔ Out`, exactly.
    fn complement(&self) -> LitSet<T> {
        match self {
            LitSet::In(set) => LitSet::Out(set.clone()),
            LitSet::Out(set) => LitSet::In(set.clone()),
        }
    }
}

/// A closed interval of integers, either end possibly unbounded (ADR-350) — the
/// refinement an `int` member carries (`int[0..]`, an index that started at zero) and
/// the LENGTH a countable member carries (`vector<int>[3]`, `list<string>[1..]`). `None`
/// at an end is that infinity. `lo > hi` is empty, and never stored: an empty meet drops
/// the tag instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub lo: Option<i64>,
    pub hi: Option<i64>,
}

impl Range {
    /// Every integer.
    pub const ALL: Range = Range { lo: None, hi: None };
    /// Exactly `n`.
    pub const fn point(n: i64) -> Range {
        Range {
            lo: Some(n),
            hi: Some(n),
        }
    }
    /// `[lo, ∞)`.
    pub const fn at_least(lo: i64) -> Range {
        Range {
            lo: Some(lo),
            hi: None,
        }
    }
    /// `(-∞, hi]`.
    pub const fn at_most(hi: i64) -> Range {
        Range {
            lo: None,
            hi: Some(hi),
        }
    }
    pub const fn new(lo: Option<i64>, hi: Option<i64>) -> Range {
        Range { lo, hi }
    }
    pub fn is_all(&self) -> bool {
        self.lo.is_none() && self.hi.is_none()
    }
    pub fn is_empty(&self) -> bool {
        matches!((self.lo, self.hi), (Some(l), Some(h)) if l > h)
    }
    /// The one value, when there is exactly one.
    pub fn as_point(&self) -> Option<i64> {
        match (self.lo, self.hi) {
            (Some(l), Some(h)) if l == h => Some(l),
            _ => None,
        }
    }
    /// The smallest range holding both — the union's WIDENING (`[0,0] ∪ [5,∞)` admits 3).
    pub fn hull(a: Range, b: Range) -> Range {
        Range {
            lo: match (a.lo, b.lo) {
                (Some(x), Some(y)) => Some(x.min(y)),
                _ => None,
            },
            hi: match (a.hi, b.hi) {
                (Some(x), Some(y)) => Some(x.max(y)),
                _ => None,
            },
        }
    }
    /// The intersection, or `None` when it is empty.
    pub fn meet(a: Range, b: Range) -> Option<Range> {
        let out = Range {
            lo: match (a.lo, b.lo) {
                (Some(x), Some(y)) => Some(x.max(y)),
                (x, None) => x,
                (None, y) => y,
            },
            hi: match (a.hi, b.hi) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (x, None) => x,
                (None, y) => y,
            },
        };
        (!out.is_empty()).then_some(out)
    }
    pub fn subset(a: Range, b: Range) -> bool {
        let lo_ok = match (a.lo, b.lo) {
            (_, None) => true,
            (Some(x), Some(y)) => x >= y,
            (None, Some(_)) => false,
        };
        let hi_ok = match (a.hi, b.hi) {
            (_, None) => true,
            (Some(x), Some(y)) => x <= y,
            (None, Some(_)) => false,
        };
        lo_ok && hi_ok
    }
    pub fn disjoint(a: Range, b: Range) -> bool {
        Range::meet(a, b).is_none()
    }
    /// Interval arithmetic, saturating: `[a,b] + [c,d] = [a+c, b+d]`, an unbounded end
    /// stays unbounded.
    pub fn plus(a: Range, b: Range) -> Range {
        Range {
            lo: a.lo.zip(b.lo).map(|(x, y)| x.saturating_add(y)),
            hi: a.hi.zip(b.hi).map(|(x, y)| x.saturating_add(y)),
        }
    }
    pub fn negated(a: Range) -> Range {
        Range {
            lo: a.hi.map(|h| h.saturating_neg()),
            hi: a.lo.map(|l| l.saturating_neg()),
        }
    }
    pub fn minus(a: Range, b: Range) -> Range {
        Range::plus(a, Range::negated(b))
    }
    /// `[a,b] × [c,d]` — the four corner products when every end is finite; else, the
    /// sign is all that survives (both non-negative → `[0, ∞)`), else everything.
    pub fn times(a: Range, b: Range) -> Range {
        match (a.lo, a.hi, b.lo, b.hi) {
            (Some(al), Some(ah), Some(bl), Some(bh)) => {
                let corners = [
                    al.saturating_mul(bl),
                    al.saturating_mul(bh),
                    ah.saturating_mul(bl),
                    ah.saturating_mul(bh),
                ];
                Range {
                    lo: corners.iter().min().copied(),
                    hi: corners.iter().max().copied(),
                }
            }
            _ => {
                let non_negative = |r: Range| r.lo.is_some_and(|l| l >= 0);
                if non_negative(a) && non_negative(b) {
                    Range::at_least(0)
                } else {
                    Range::ALL
                }
            }
        }
    }
    /// The WIDENING of an ascent: an end that moved outward since `prev` goes to its
    /// infinity, so a loop counter's `[0,0], [0,1], [0,2], …` is `[0, ∞)` in one step.
    pub fn widen(prev: Range, next: Range) -> Range {
        Range {
            lo: match (prev.lo, next.lo) {
                (Some(p), Some(n)) if n < p => None,
                (None, _) => None,
                (_, n) => n,
            },
            hi: match (prev.hi, next.hi) {
                (Some(p), Some(n)) if n > p => None,
                (None, _) => None,
                (_, n) => n,
            },
        }
    }
}

/// The tags whose members have a LENGTH — what `count` measures: a list (`pair`; the
/// empty list is `nil`, always 0, and carries none), a vector, a set, a map, a string, a
/// `bytes`. The `len` slot describes every such member of a term at once.
const COUNT_BITS: u32 = SEQ_BITS | MAP_BIT | STR_BIT | BYTES_BIT;

/// A literal slot in canonical form. Two spellings of the same set must not survive as
/// different `Ty`s: `Ty` derives its equality and hash from the slots, so `false | true`
/// comparing unequal to `bool` is not cosmetic — it makes `bool <: (or false true)` come
/// out **false** for two identical sets, which is a spurious warning waiting to happen,
/// and it breaks the fixpoint loops that iterate until a type stops changing.
///
/// Two ways a slot can say "every value of the tag", which is what `None` means:
/// excluding nothing, and — for **bool** alone, the one finite domain — listing both.
fn canon_lit<T: Ord>(slot: Option<Arc<LitSet<T>>>) -> Option<Arc<LitSet<T>>> {
    match slot.as_deref() {
        Some(LitSet::Out(excluded)) if excluded.is_empty() => None,
        _ => slot,
    }
}

/// [`canon_lit`] plus bool's finite-domain rule: `{true, false}` is every bool.
fn canon_lit_bool(slot: Option<Arc<LitSet<bool>>>) -> Option<Arc<LitSet<bool>>> {
    let slot = canon_lit(slot);
    match slot.as_deref() {
        Some(LitSet::In(set)) if set.len() == 2 => None,
        _ => slot,
    }
}

/// Union of two sets over the same tag. `None` on either side means "every value of the
/// tag", which absorbs.
fn lit_union<T: Ord + Clone>(a: Option<&LitSet<T>>, b: Option<&LitSet<T>>) -> Option<LitSet<T>> {
    match (a?, b?) {
        (LitSet::In(x), LitSet::In(y)) => Some(LitSet::In(x.union(y).cloned().collect())),
        (LitSet::Out(x), LitSet::Out(y)) => Some(LitSet::Out(x.intersection(y).cloned().collect())),
        // `In(A) ∪ Out(B)` = everything but `B∖A`: the excluded values `A` puts back are
        // no longer excluded.
        (LitSet::In(x), LitSet::Out(y)) | (LitSet::Out(y), LitSet::In(x)) => {
            Some(LitSet::Out(y.difference(x).cloned().collect()))
        }
    }
    .filter(|set| !matches!(set, LitSet::Out(e) if e.is_empty()))
}

/// Intersection of two sets over the same tag. `None` means "every value", the identity.
fn lit_intersect<T: Ord + Clone>(
    a: Option<&LitSet<T>>,
    b: Option<&LitSet<T>>,
) -> Option<LitSet<T>> {
    match (a, b) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (Some(LitSet::In(x)), Some(LitSet::In(y))) => {
            Some(LitSet::In(x.intersection(y).cloned().collect()))
        }
        (Some(LitSet::Out(x)), Some(LitSet::Out(y))) => {
            Some(LitSet::Out(x.union(y).cloned().collect()))
        }
        (Some(LitSet::In(x)), Some(LitSet::Out(y)))
        | (Some(LitSet::Out(y)), Some(LitSet::In(x))) => {
            Some(LitSet::In(x.difference(y).cloned().collect()))
        }
    }
}

/// Is every value `a` admits for the tag one that `b` admits?
fn lit_subset<T: Ord>(a: Option<&LitSet<T>>, b: Option<&LitSet<T>>) -> bool {
    let Some(b) = b else {
        return true; // `b` admits every value of the tag
    };
    match (a, b) {
        // `a` is every value of the tag; `b` is not (it is `Some`), so no.
        (None, _) => false,
        (Some(LitSet::In(x)), LitSet::In(y)) => x.is_subset(y),
        (Some(LitSet::In(x)), LitSet::Out(y)) => x.is_disjoint(y),
        // An infinite domain minus finitely many is never inside a finite listing.
        (Some(LitSet::Out(_)), LitSet::In(_)) => false,
        (Some(LitSet::Out(x)), LitSet::Out(y)) => y.is_subset(x),
    }
}

/// Do `a` and `b` share no value of the tag? `None` = every value, which shares with
/// anything non-empty.
fn lit_sets_disjoint<T: Ord>(a: Option<&LitSet<T>>, b: Option<&LitSet<T>>) -> bool {
    match (a, b) {
        (None, _) | (_, None) => false,
        (Some(LitSet::In(x)), Some(LitSet::In(y))) => x.is_disjoint(y),
        (Some(LitSet::In(x)), Some(LitSet::Out(y)))
        | (Some(LitSet::Out(y)), Some(LitSet::In(x))) => x.is_subset(y),
        // Two infinite complements always overlap.
        (Some(LitSet::Out(_)), Some(LitSet::Out(_))) => false,
    }
}

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
    /// **The self-reference of a recursive type** (2026-09-14, ADR-349). A term with this
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
    /// `⊥` — the empty set; the type of no value. A subtype of every type.
    pub const NEVER: Ty = Ty::flat(0);
    /// `⊤` — every tag; the type of any value. A supertype of every type.
    pub const ANY: Ty = Ty::flat(UNIVERSE);
    /// `int ∪ float ∪ decimal` — the named union the prelude's `number?` predicate
    /// implies. A `decimal` is a number (but not an integer).
    pub const NUMBER: Ty = Ty::flat(
        (1u32 << bit(Tag::Int))
            | (1u32 << bit(Tag::Float))
            | (1u32 << bit(Tag::Decimal))
            | (1u32 << bit(Tag::Ratio)),
    );
    /// `nil ∪ pair` — the named union the prelude's `list?` predicate implies.
    pub const LIST: Ty = Ty::flat((1u32 << bit(Tag::Nil)) | (1u32 << bit(Tag::Pair)));
    /// The **seqable** union — every collection the sequence combinators walk (a list —
    /// `nil`/`pair`, a range/seqview reading as `pair` — a vector, set, map, or `bytes`;
    /// `string` is deliberately excluded). The named type a polymorphic-sequence `sig`
    /// parameter uses (`(sig f (seqable -> …))`) instead of falling back to `any`.
    pub const SEQABLE: Ty = Ty::flat(
        (1u32 << bit(Tag::Nil))
            | (1u32 << bit(Tag::Pair))
            | (1u32 << bit(Tag::Vector))
            | (1u32 << bit(Tag::Set))
            | (1u32 << bit(Tag::Map))
            | (1u32 << bit(Tag::Bytes)),
    );

    /// The **countable** union — everything `count`/`get`/`empty?` accept: the seqable
    /// collections plus the three sized non-sequences, `string`, `rope` and `table`. The
    /// named type of `count`'s parameter, so a function that counts its argument is
    /// suggested `(countable -> …)` rather than the six-way union spelled out.
    pub const COUNTABLE: Ty = Ty::flat(
        (1u32 << bit(Tag::Nil))
            | (1u32 << bit(Tag::Pair))
            | (1u32 << bit(Tag::Vector))
            | (1u32 << bit(Tag::Set))
            | (1u32 << bit(Tag::Map))
            | (1u32 << bit(Tag::Bytes))
            | (1u32 << bit(Tag::Str))
            | (1u32 << bit(Tag::Rope))
            | (1u32 << bit(Tag::Table)),
    );

    /// Every truthy value — `any ∖ nil ∖ false`: what an `(if x …)` / `(when x …)` /
    /// `(or x …)` guard leaves of an unknown.
    pub fn truthy() -> Ty {
        Ty::ANY
            .difference(Ty::of(Tag::Nil))
            .difference(Ty::bool_lit(false))
    }

    /// Is this type known only by what it is NOT — `any`, or `any` less a guard's
    /// `nil`/`false`? Such a bound says nothing positive about the value, so strict
    /// checking (`GradualTy::consistent_with_mode`) keeps the overlap reading for it.
    pub fn is_known_only_by_exclusion(&self) -> bool {
        // An INTERVAL (ADR-350) — a length a guard established, an int's bounds — does not
        // make a term positively known: `(not (nil | false))` of length at least one is
        // still "not nil", so the question is asked of the term without its intervals.
        let this = self.without_intervals();
        let this = &this;
        if Ty::truthy().is_subtype(this) {
            return true;
        }
        // `any ∖ (a finite literal set)` — what a failed `(= x 0)` / `(= tag :done)` leaves
        // of an unknown: it says which values `x` is NOT, and nothing it is. Reading
        // `(not 0)` by inclusion flagged `(- i 1)` in every loop that tested `(= i 0)` first.
        let excluded = Ty::ANY.difference(this.clone());
        let without_falsy = excluded
            .difference(Ty::of(Tag::Nil))
            .difference(Ty::of(Tag::Bool));
        if without_falsy.is_never()
            || without_falsy.as_lit().is_some()
            || without_falsy.as_lit_int().is_some()
            || without_falsy.as_lit_str().is_some()
        {
            return true;
        }
        // `any ∖ vector` — what a failed `(vector? x)` leaves: a flat tag set with no
        // positive refinement, describing MORE than half the universe. Its complement is
        // the shorter description, i.e. what is known is what the value is not.
        let refined = this.alts.is_some()
            || this.arrow.is_some()
            || this.elem.is_some()
            || this.map_kv.is_some()
            || this.fields.is_some()
            || this.tuple.is_some()
            || this.lit.as_deref().is_some_and(|l| l.members().is_some())
            || this
                .lit_int
                .as_deref()
                .is_some_and(|l| l.members().is_some())
            || this
                .lit_bool
                .as_deref()
                .is_some_and(|l| l.members().is_some())
            || this
                .lit_str
                .as_deref()
                .is_some_and(|l| l.members().is_some());
        !refined && this.tags.count_ones() * 2 > UNIVERSE.count_ones()
    }

    /// The record identities named by this type's map member (`:t/usd`, …), as spelled
    /// in a `sig` — `None` when the map member is not a nominal shape.
    pub fn project_record_ids(&self) -> Option<Vec<String>> {
        let fields = self.record_fields()?;
        display::nominal_ids(fields)
    }

    /// A flat (unrefined) type from a raw tag bitset — the internal constructor
    /// every flat `Ty` funnels through. `const` so the named points above can be
    /// `const`; the set operations that combine refinements can't be.
    const fn flat(tags: u32) -> Ty {
        Ty {
            tags,
            neg: None,
            arrow: None,
            elem: None,
            map_kv: None,
            overload: None,
            fields: None,
            tuple: None,
            list_shape: None,
            lit: None,
            lit_int: None,
            lit_bool: None,
            lit_str: None,
            alts: None,
            rec_ref: false,
            mu: false,
            int_range: None,
            len: None,
        }
    }

    /// Does this type carry **no** refinements — is it exactly its tag set? The
    /// question the complement rendering and the DNF-free fast paths ask: a flat type
    /// is fully described by `tags`, so set reasoning on it is exact.
    pub fn is_flat(&self) -> bool {
        // A union of terms is not flat even when its *head* carries no refinement —
        // the alternatives do the describing. Without this the predicate answers about
        // one term while reading as if it answered about the type.
        self.alts.is_none()
            && self.arrow.is_none()
            && self.overload.is_none()
            && self.elem.is_none()
            && self.map_kv.is_none()
            && self.fields.is_none()
            && self.tuple.is_none()
            && self.lit.is_none()
            && self.lit_int.is_none()
            && self.lit_bool.is_none()
            && self.lit_str.is_none()
    }

    /// The singleton type containing exactly the values with this tag.
    pub const fn of(tag: Tag) -> Ty {
        Ty::flat(1u32 << bit(tag))
    }

    /// The flat union of several tags — `const`, so callers can build named
    /// shorthands (e.g. `seq = nil | pair | vector`) as `const` items without the
    /// non-`const` [`union`](Ty::union). Unrefined (every flat type is).
    pub const fn of_tags(tags: &[Tag]) -> Ty {
        let mut bits = 0u32;
        let mut i = 0;
        while i < tags.len() {
            bits |= 1u32 << bit(tags[i]);
            i += 1;
        }
        Ty::flat(bits)
    }

    /// A function type `(params...) -> ret` — the function members refined to
    /// exactly those matching `sig`. Tagged `Fn|Native` (an arrow describes both
    /// closures and builtins).
    pub fn arrow(sig: Sig) -> Ty {
        Ty {
            arrow: Some(Arc::new(sig)),
            ..Ty::flat(FN_BITS)
        }
    }

    /// The function-arrow refinement, if this type carries one. The bridge the
    /// advisory checker reads to compare a callback against what a higher-order
    /// function expects.
    pub fn as_arrow(&self) -> Option<&Sig> {
        self.single()?.arrow.as_deref()
    }

    /// An overloaded function type — `sigs[0] and sigs[1] and …` (an
    /// intersection of 2+ distinct arrows), e.g. `(int -> int) and (bool ->
    /// bool)`. Tagged `Fn|Native` like a plain arrow. `sigs` must have at
    /// least 2 elements; a single signature belongs in [`Ty::arrow`] instead
    /// — [`Ty::intersect`] enforces this collapse automatically, so this
    /// constructor is for tests/direct construction only.
    pub fn overload_of(sigs: Vec<Sig>) -> Ty {
        Ty {
            overload: Some(Arc::new(sigs)),
            ..Ty::flat(FN_BITS)
        }
    }

    /// The overload refinement, if this type carries one — the bridge the
    /// checker reads to resolve a call's return type per matching arm. `None`
    /// when this type carries at most a single [`Ty::arrow`].
    pub fn overload_sigs(&self) -> Option<&Vec<Sig>> {
        self.single()?.overload.as_deref()
    }

    /// A sequence type over `tags` (some subset of `pair`/`vector`) whose elements
    /// have type `elem` — the general element-refinement constructor.
    pub fn seq_of(tags: u32, elem: Ty) -> Ty {
        Ty {
            elem: Some(Arc::new(elem)),
            ..Ty::flat(tags & SEQ_BITS)
        }
        .bounded()
    }

    /// `map<K, V>` — a map whose keys have type `K` and values have type `V`.
    pub fn map_of(key: Ty, val: Ty) -> Ty {
        Ty {
            map_kv: Some(Arc::new((key, val))),
            ..Ty::flat(MAP_BIT)
        }
        .bounded()
    }

    /// A heterogeneous record shape — `field name → (declared type,
    /// required?)`. Tagged `map` (a record is still a runtime `map` value;
    /// this only refines it, the same trick [`Ty::keyword_lit`] uses layering
    /// onto the `Keyword` tag). See `docs/type-records.md`.
    /// A **closed** record shape — `(record :a int)`. Exactly the declared fields;
    /// every other key is absent, which reads as `nil` (ADR-264).
    pub fn record_of(fields: BTreeMap<Symbol, (Ty, bool)>) -> Ty {
        Ty::record_shape(fields, Ty::of(Tag::Nil))
    }

    /// An **open** record shape — `(record &open :a int)`. The declared fields, and a
    /// value may carry any others. This is what a shape used as a *parameter domain*
    /// wants (a `defrecord` accessor takes any record carrying its field), and what an
    /// ability-as-a-type resolves to.
    pub fn record_of_open(fields: BTreeMap<Symbol, (Ty, bool)>) -> Ty {
        Ty::record_shape(fields, Ty::ANY)
    }

    /// The general constructor: declared fields plus the type of every undeclared key.
    pub fn record_shape(fields: BTreeMap<Symbol, (Ty, bool)>, rest: Ty) -> Ty {
        Ty {
            fields: Some(Arc::new(RecordShape { fields, rest })),
            ..Ty::flat(MAP_BIT)
        }
        .bounded()
    }

    /// The record-shape refinement's declared fields, if this map type carries one.
    pub fn record_fields(&self) -> Option<&BTreeMap<Symbol, (Ty, bool)>> {
        Some(&self.single()?.fields.as_deref()?.fields)
    }

    /// The type a `(get r k)` yields on this record type, or `None` when there is no
    /// answer to give. Unlike [`Ty::record_fields`] this answers for **every** key: an
    /// undeclared key on a closed record is `nil` (it is absent), which is what lets a
    /// field read through a tagged union resolve at all.
    ///
    /// Over a *union* of shapes it is the union of what each term yields — the whole
    /// point of the closed shape: `{ok: int} | {error: string}` answers `int | nil` for
    /// `:ok`, because the second term says the key is absent rather than shrugging.
    pub fn record_field_ty(&self, key: Symbol) -> Option<Ty> {
        if self.mu {
            return self.unroll().record_field_ty(key);
        }
        // The single-term case is the common one and must not pay for the union: it
        // reads the shape in place rather than materialising a term vector (this is a
        // per-field-read hot path in the checker).
        if self.alts.is_none() {
            return self.term_field_ty_or_nil(key);
        }
        let mut acc: Option<Ty> = None;
        for term in self.terms_vec() {
            let t = term.term_field_ty_or_nil(key)?;
            acc = Some(match acc {
                Some(a) => a.union(t),
                None => t,
            });
        }
        acc
    }

    /// One term's field reading, with the `nil` member folded in: `(:k nil)` and `(get nil
    /// :k)` answer `nil` at runtime, so `(:k maybe-record)` over `nil | {k: int}` is
    /// `nil | int` — the shape every optional field read has. A `nil | {…}` union is ONE
    /// term (`nil` carries no refinement to keep it apart), tagged `nil|map` with the
    /// record's shape, and reading it used to demand `tags == map` and give up — so the
    /// whole read was UNKNOWN and every consumer downstream of an optional record lost
    /// the field types the record had declared. Any other non-map tag (a vector, a string
    /// — a keyword read raises on those) still declines.
    fn term_field_ty_or_nil(&self, key: Symbol) -> Option<Ty> {
        if self.tags & !(MAP_BIT | NIL_BIT) != 0 {
            return None;
        }
        let from_map = if self.tags & MAP_BIT != 0 {
            Some(self.term_field_ty(key)?)
        } else {
            None
        };
        let from_nil = (self.tags & NIL_BIT != 0).then(|| Ty::of(Tag::Nil));
        match (from_map, from_nil) {
            (Some(m), Some(n)) => Some(m.union(n)),
            (Some(m), None) => Some(m),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    /// [`record_field_ty`](Self::record_field_ty) for `(get r k default)`: the absence
    /// case reads as `default`'s type rather than `nil`. See [`RecordShape::field_ty_with_default`].
    pub fn record_field_ty_with_default(&self, key: Symbol, default: &Ty) -> Option<Ty> {
        let mut acc: Option<Ty> = None;
        for term in self.terms_vec() {
            // The `nil` member reads as the default — `(get nil :k 7)` is `7` — see
            // `term_field_ty_or_nil` for why a `nil | {…}` union is one term.
            if term.tags & !(MAP_BIT | NIL_BIT) != 0 {
                return None;
            }
            let from_nil = (term.tags & NIL_BIT != 0).then(|| default.clone());
            let from_map = if term.tags & MAP_BIT != 0 {
                let shape = term.fields.as_deref()?;
                Some(if !shape.fields.contains_key(&key) && shape.is_open() {
                    match term.map_kv.as_deref() {
                        Some((_, v)) => v.clone().union(default.clone()),
                        None => shape.field_ty_with_default(key, default.clone()),
                    }
                } else {
                    shape.field_ty_with_default(key, default.clone())
                })
            } else {
                None
            };
            let t = match (from_map, from_nil) {
                (Some(m), Some(n)) => m.union(n),
                (Some(m), None) => m,
                (None, Some(n)) => n,
                (None, None) => return None,
            };
            acc = Some(match acc {
                Some(a) => a.union(t),
                None => t,
            });
        }
        acc
    }

    /// One term's answer for `key`. Almost always the shape's own reading — except for
    /// an **undeclared** key on an **open** shape, where the shape says only `any` and a
    /// `map<K,V>` refinement on the same term (an intersection can carry both) says
    /// something sharper. Taking the shape's `any` there would be a precision
    /// regression against the pre-shape behaviour, which consulted `map_kv`.
    fn term_field_ty(&self, key: Symbol) -> Option<Ty> {
        let shape = self.fields.as_deref()?;
        if !shape.fields.contains_key(&key) && shape.is_open() {
            if let Some((_, v)) = self.map_kv.as_deref() {
                return Some(v.clone().union(Ty::of(Tag::Nil)));
            }
        }
        Some(shape.field_ty(key))
    }

    /// Is this a record shape that admits keys it does not declare?
    pub fn record_is_open(&self) -> Option<bool> {
        Some(self.single()?.fields.as_deref()?.is_open())
    }

    /// A positional tuple shape — one type per index, fixed arity. Tagged
    /// `vector` (a tuple is still a runtime `[ ]` vector value; this only
    /// refines it, the same layering trick `record_of` uses onto `map`). See
    /// `docs/type-tuples.md` (ADR-128).
    pub fn tuple_of(elems: Vec<Ty>) -> Ty {
        Ty {
            tuple: Some(Arc::new(elems)),
            ..Ty::flat(VECTOR_BIT)
        }
        .bounded()
    }

    /// A positional LIST shape — a non-empty list of exactly these elements, one type
    /// per index. Tagged `pair` alone (the empty list is `nil`, a separate tag); the
    /// list sibling of [`Ty::tuple_of`]. `elems` must be non-empty.
    pub fn list_shape_of(elems: Vec<Ty>) -> Ty {
        debug_assert!(!elems.is_empty(), "a list shape has at least one element");
        Ty {
            list_shape: Some(Arc::new(elems)),
            ..Ty::flat(PAIR_BIT)
        }
        .bounded()
    }

    /// The list-shape refinement, if this type is nothing but a list of a known shape —
    /// the same refusal [`Ty::tuple_elems`] makes: a term that also admits `nil` or a
    /// vector has no positional answer.
    pub fn list_shape_elems(&self) -> Option<&Vec<Ty>> {
        let this = self.single()?;
        if this.tags != PAIR_BIT {
            return None;
        }
        this.list_shape.as_deref()
    }

    /// The positions of a type that is exactly a vector tuple or exactly a list shape —
    /// what `(first x)` / `(nth x <literal>)` / `(rest x)` read, since each answers the
    /// same way for either sequence kind.
    pub fn positional_elems(&self) -> Option<Vec<Ty>> {
        if self.mu {
            return self.unroll().positional_elems();
        }
        self.tuple_elems()
            .or_else(|| self.list_shape_elems())
            .cloned()
    }

    /// The tuple-shape refinement, if this vector type carries one. The
    /// bridge the checker reads to flow `(nth t i)`/`(first t)` to the exact
    /// per-position type.
    pub fn tuple_elems(&self) -> Option<&Vec<Ty>> {
        let this = self.single()?;
        // Positional access is exact ONLY on a term that is nothing but a vector of this
        // shape: a consumer reads "position 0 is `0`, and never nil". A term that also
        // admits a `pair` (unknown elements) or `nil` (`first` is nil) has no positional
        // answer at all — reporting the tuple's was how `(tuple 0) ∪ pair` typed `first`
        // as `0` on a value that was a string.
        if this.tags != VECTOR_BIT {
            return None;
        }
        this.tuple.as_deref()
    }

    /// The type at tuple position `n` — exact over a UNION of tuple terms too (2026-09-12):
    /// `(tuple 0 nil) | (tuple int pair)` at position 0 is `0 | int`, since each term is
    /// nothing but a vector of a known shape and position `n` of the union is the union of
    /// the positions. `None` when any term is not such a tuple, or is too short — the same
    /// refusal [`Ty::tuple_elems`] makes for one term. What a destructuring `[j acc] st`
    /// over a fold accumulator reads: without this the union fell to `elem_ty` — every
    /// element of every term — and `j` came out `nil | int | pair`, which `inc` widened to
    /// `number` (a strict false positive in `std/editor/markdown`).
    pub fn tuple_elem_at(&self, n: usize) -> Option<Ty> {
        if self.mu {
            return self.unroll().tuple_elem_at(n);
        }
        let terms = self.terms_vec();
        let mut out = Ty::NEVER;
        for term in &terms {
            if term.tags != VECTOR_BIT {
                return None;
            }
            let elem = term.tuple.as_deref()?.get(n)?.clone();
            out = out.union(elem);
        }
        Some(out)
    }

    /// A keyword-literal (singleton) type — exactly the keyword `sym`. Unions of
    /// these build an enumerated keyword type, e.g. `(or :maximized :fullboth)`.
    pub fn keyword_lit(sym: Symbol) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(sym);
        Ty {
            lit: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(KEYWORD_BIT)
        }
    }

    /// The keyword-literal refinement, if this type carries one (the exact keyword
    /// symbols admitted). `None` means "any keyword" (or no keyword member).
    pub fn as_lit(&self) -> Option<&BTreeSet<Symbol>> {
        self.single()?.lit.as_deref()?.members()
    }

    /// An int-literal (singleton) type — exactly the integer `n` (ADR-117).
    /// Unions of these build an enumerated int type, e.g. `(or 200 404 500)`.
    /// Independent of [`Ty::keyword_lit`] (a different tag), so the two
    /// compose freely — `(or :ok 5)` carries both refinements at once.
    pub fn int_lit(n: i64) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(n);
        Ty {
            lit_int: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(INT_BIT)
        }
    }

    /// The int-literal refinement, if this type carries one (the exact
    /// integers admitted). `None` means "any int" (or no int member).
    pub fn as_lit_int(&self) -> Option<&BTreeSet<i64>> {
        self.single()?.lit_int.as_deref()?.members()
    }

    /// `int` within `range` (ADR-350): `Ty::int_in(Range::at_least(0))` is `int[0..]`.
    pub fn int_in(range: Range) -> Ty {
        if let Some(n) = range.as_point() {
            return Ty::int_lit(n);
        }
        Ty {
            int_range: (!range.is_all()).then_some(range),
            ..Ty::flat(INT_BIT)
        }
    }

    /// This type with its countable members' LENGTH refined to `range` — `(vector int)`
    /// of length 3, a non-empty list. A term with no countable member is unchanged; a
    /// length of exactly 0 drops the `pair` member (a list of length 0 is `nil`).
    pub fn with_len(self, range: Range) -> Ty {
        let terms: Vec<Ty> = self
            .terms_vec()
            .into_iter()
            .map(|mut term| {
                if term.mu || term.tags & COUNT_BITS == 0 {
                    return term;
                }
                // `nil` counts 0: a length that excludes 0 excludes it.
                if !Range::subset(Range::point(0), range) {
                    term.tags &= !NIL_BIT;
                }
                match Range::meet(term.len_eff(), range) {
                    None => term.tags &= !COUNT_BITS,
                    Some(r) => {
                        if r.hi == Some(0) {
                            term.tags &= !PAIR_BIT;
                        }
                        term.len = canon_len(term.tags, Some(r));
                        term = term.normalise_len();
                    }
                }
                term
            })
            .collect();
        let mu = self.mu;
        let mut out = Ty::from_terms(terms);
        if mu {
            out.mu = true;
        }
        out
    }

    /// This type with its int member OPENED — no literal set, no interval — for a result
    /// whose operand COUNT is unknown: `(apply + xs)` and a numeric fold stay in `+`'s
    /// closure (an int) but not in one step's interval (ADR-350).
    pub fn without_int_interval(&self) -> Ty {
        let terms: Vec<Ty> = self
            .terms_vec()
            .into_iter()
            .map(|mut term| {
                if term.tags & INT_BIT != 0 && !term.mu {
                    term.lit_int = None;
                    term.int_range = None;
                }
                term
            })
            .collect();
        let mut out = Ty::from_terms(terms);
        out.mu = self.mu;
        out
    }

    /// This type with every interval dropped — the int bounds and the lengths — on every
    /// term. What an interval-blind reading of the type sees.
    pub fn without_intervals(&self) -> Ty {
        let terms: Vec<Ty> = self
            .terms_vec()
            .into_iter()
            .map(|mut term| {
                if !term.mu {
                    term.int_range = None;
                    term.len = None;
                }
                term
            })
            .collect();
        let mut out = Ty::from_terms(terms);
        out.mu = self.mu;
        out
    }

    /// The interval every int this type admits lies in: the literal set's span when
    /// it is positive, else the interval refinement, else everything. Over a union, the
    /// hull. `None` when the type admits no int at all.
    pub fn int_range(&self) -> Option<Range> {
        let mut out: Option<Range> = None;
        for term in self.terms_vec() {
            if term.tags & INT_BIT == 0 {
                continue;
            }
            let r = term.int_range_eff();
            out = Some(match out {
                Some(acc) => Range::hull(acc, r),
                None => r,
            });
        }
        out
    }

    /// One term's effective int interval (see [`Ty::int_range`]).
    fn int_range_eff(&self) -> Range {
        if let Some(LitSet::In(set)) = self.lit_int.as_deref() {
            if let (Some(lo), Some(hi)) = (set.iter().next(), set.iter().next_back()) {
                return Range::new(Some(*lo), Some(*hi));
            }
        }
        self.int_range.unwrap_or(Range::ALL)
    }

    /// The interval `(count x)` lies in for every value of this type: the length of the
    /// countable members (a `pair` is at least 1), `0` for `nil`, the hull over a union.
    /// `None` when the type has no countable member and no `nil` (nothing to count).
    pub fn count_range(&self) -> Option<Range> {
        let mut out: Option<Range> = None;
        let mut fold = |r: Range| {
            out = Some(match out {
                Some(acc) => Range::hull(acc, r),
                None => r,
            })
        };
        for term in self.terms_vec() {
            if term.tags & COUNT_BITS != 0 {
                fold(term.len_eff());
            }
            if term.tags & NIL_BIT != 0 {
                fold(Range::point(0));
            }
        }
        out
    }

    /// One term's effective length interval: the slot, else `[1, ∞)` when the only
    /// countable member is a `pair`, else everything.
    fn len_eff(&self) -> Range {
        if let Some(r) = self.len {
            return r;
        }
        // A positional shape IS its arity, when it is the only countable member.
        if self.tags & COUNT_BITS == VECTOR_BIT {
            if let Some(ts) = &self.tuple {
                return Range::point(ts.len() as i64);
            }
        }
        if self.tags & COUNT_BITS == PAIR_BIT {
            if let Some(ts) = &self.list_shape {
                return Range::point(ts.len() as i64);
            }
            return Range::at_least(1);
        }
        Range::at_least(0)
    }

    /// This type's WIDENING against the value it had a round earlier (ADR-350): every
    /// interval — an int's, a length's — whose end moved outward goes to its infinity,
    /// so a counter's ascent `[0,0], [0,1], …` is `[0, ∞)` in one step. Walks the
    /// refinement trees in parallel where they have the same shape; leaves the rest.
    pub fn widen_intervals_against(&self, prev: &Ty) -> Ty {
        // Alternatives that share their tags are merged first — by the widening merge —
        // on both sides, WHEN the alternatives are multiplying: a tuple accumulator whose
        // steps differ in two positions (`(tuple 0 nil) | (tuple 1 (list 0)) | …`) grows
        // an alternative per round, and is widened as one shape instead. A union that is
        // not growing keeps its alternatives — `(tuple model int) | (tuple model' int)`,
        // two branches returning a pair, is not an ascent, and merging it lost the tuple
        // shape (a destructuring `[m idx]` then read `int | map`). A record alternative is
        // left alone either way: the tagged-union idiom (`{:ok v} | {:error e}`) must not
        // merge under a fixpoint.
        let (my_terms, their_terms) = (self.terms_vec(), prev.terms_vec());
        let growing = my_terms.len() > their_terms.len();
        let (mine, theirs) = if growing {
            (
                collapse_same_tags(my_terms),
                collapse_same_tags(their_terms),
            )
        } else {
            (my_terms, their_terms)
        };
        let widened: Vec<Ty> = mine
            .into_iter()
            .map(|term| {
                // The previous term with the same tags (`nil` aside — it rides along with
                // whichever alternative it merged into), if there is exactly one.
                let mut matching = theirs
                    .iter()
                    .filter(|p| p.tags & !NIL_BIT == term.tags & !NIL_BIT);
                let (Some(prev_term), None) = (matching.next(), matching.next()) else {
                    return term;
                };
                let mut out = term.clone();
                // A literal SET that grew is an interval on the move too — `0`, `0 | 1`,
                // `0 | 1 | 2` is a counter — so it widens the same way, to an interval.
                let lit_grew = match (term.lit_int.as_deref(), prev_term.lit_int.as_deref()) {
                    (Some(LitSet::In(now)), Some(LitSet::In(before))) => {
                        now.len() > before.len() && before.is_subset(now)
                    }
                    (Some(LitSet::In(_)), None) => prev_term.int_range.is_some(),
                    _ => false,
                };
                if term.tags & INT_BIT != 0
                    && (term.int_range.is_some() || prev_term.int_range.is_some() || lit_grew)
                {
                    let r = Range::widen(prev_term.int_range_eff(), term.int_range_eff());
                    if lit_grew {
                        out.lit_int = None;
                    }
                    out.int_range = canon_int_range(&out.lit_int, Some(r));
                }
                if term.tags & COUNT_BITS != 0 && (term.len.is_some() || prev_term.len.is_some()) {
                    let r = Range::widen(prev_term.len_eff(), term.len_eff());
                    out.len = canon_len(out.tags, Some(r));
                    out = out.normalise_len();
                }
                if !out.mu {
                    out = out.map_nested_with(prev_term, &mut |t, p| t.widen_intervals_against(p));
                }
                out
            })
            .collect();
        let mut out = Ty::from_terms(widened);
        out.mu = self.mu;
        out
    }

    /// [`Ty::map_nested`] over two terms of the same shape: `f` sees each nested Ty of
    /// `self` beside the corresponding one of `other` where `other` has it, and the
    /// nested Ty alone (unchanged) where it does not.
    fn map_nested_with(&self, other: &Ty, f: &mut dyn FnMut(&Ty, &Ty) -> Ty) -> Ty {
        let mut out = self.clone();
        if let (Some(e), Some(p)) = (&self.elem, &other.elem) {
            out.elem = Some(Arc::new(f(e, p)));
        }
        if let (Some(kv), Some(pkv)) = (&self.map_kv, &other.map_kv) {
            out.map_kv = Some(Arc::new((f(&kv.0, &pkv.0), f(&kv.1, &pkv.1))));
        }
        if let (Some(shape), Some(pshape)) = (&self.fields, &other.fields) {
            out.fields = Some(Arc::new(RecordShape {
                fields: shape
                    .fields
                    .iter()
                    .map(|(k, (t, req))| {
                        let t = match pshape.fields.get(k) {
                            Some((p, _)) => f(t, p),
                            None => t.clone(),
                        };
                        (*k, (t, *req))
                    })
                    .collect(),
                rest: f(&shape.rest, &pshape.rest),
            }));
        }
        for (mine, theirs) in [
            (&mut out.tuple, &other.tuple),
            (&mut out.list_shape, &other.list_shape),
        ] {
            if let (Some(ts), Some(ps)) = (mine.as_deref(), theirs.as_deref()) {
                if ts.len() == ps.len() {
                    *mine = Some(Arc::new(
                        ts.iter().zip(ps.iter()).map(|(t, p)| f(t, p)).collect(),
                    ));
                }
            }
        }
        out
    }

    /// A bool-literal (singleton) type — exactly `true` or `false` (ADR-120).
    /// Unlike the keyword-literal era's guidance ("`false` isn't a literal
    /// type"), that restriction was specific to avoiding `false`/`nil`
    /// confusion in an *enumerated keyword* set — now that bool-literal types
    /// are their own real kind, both values are legitimate singletons.
    pub fn bool_lit(b: bool) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(b);
        Ty {
            lit_bool: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(BOOL_BIT)
        }
    }

    /// The bool-literal refinement, if this type carries one. `None` means
    /// "any bool" (or no bool member).
    pub fn as_lit_bool(&self) -> Option<&BTreeSet<bool>> {
        self.single()?.lit_bool.as_deref()?.members()
    }

    /// A string-literal (singleton) type — exactly the string `s` (ADR-120).
    /// Takes `&str` rather than a `Value`/`Heap` pair — the caller reads the
    /// content out of its `Value::Str` heap handle first (`heap.string(id)`),
    /// so `Ty` itself stays heap-independent like every other constructor.
    pub fn str_lit(s: &str) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(s.to_string());
        Ty {
            lit_str: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(STR_BIT)
        }
    }

    /// The string-literal refinement, if this type carries one. `None` means
    /// "any string" (or no string member).
    pub fn as_lit_str(&self) -> Option<&BTreeSet<String>> {
        self.single()?.lit_str.as_deref()?.members()
    }

    /// The key/value refinement, if this map type carries one. The bridge the
    /// checker reads to flow `(get m k)` → `V | nil`, `(keys m)` → `list<K>`, etc.
    pub fn map_kv(&self) -> Option<(Ty, Ty)> {
        if self.mu {
            return self.unroll().map_kv();
        }
        self.single()?.map_kv.as_deref().cloned()
    }

    /// `vector<elem>` — a vector whose elements have type `elem`.
    pub fn vector_of(elem: Ty) -> Ty {
        Ty::seq_of(1u32 << bit(Tag::Vector), elem)
    }

    /// `list<elem>` — a (non-empty) list whose elements have type `elem`. Tagged
    /// `pair`; the empty-list `nil` carries no element type, so a value that may
    /// be `nil` widens to plain `list` at the join.
    pub fn list_of(elem: Ty) -> Ty {
        Ty::seq_of(1u32 << bit(Tag::Pair), elem)
    }

    /// `set<elem>` — a set every element of which is an `elem`. Like a vector (and
    /// unlike a list, whose empty case is `nil`), the empty set inhabits every
    /// `set<T>`, so `set<never>` is the type of `#{}` and never uninhabited.
    pub fn set_of(elem: Ty) -> Ty {
        Ty::seq_of(1u32 << bit(Tag::Set), elem)
    }

    /// [`Ty::elem_ty`] over a UNION too: the union of the terms' elements — sound, since a
    /// value of the union is a value of one term — when every term that is a collection
    /// says what it holds (a `nil` term holds nothing and is skipped; a non-collection
    /// term means the value may not be a collection at all, so there is no answer). For
    /// the combinators that walk a value's elements (`map`, `reverse`, `rest`, `cons`
    /// onto it); a POSITIONAL read (`nth`) wants [`Ty::element_at_over_union`] instead.
    pub fn elem_ty_union(&self) -> Option<Ty> {
        if self.alts.is_none() {
            return self.elem_ty();
        }
        let mut out = Ty::NEVER;
        for term in self.terms_vec() {
            if term.tags == NIL_BIT {
                continue;
            }
            out = out.union(term.elem_ty()?);
        }
        Some(out)
    }

    /// This type with only the terms `keep` admits — `never` when none survives.
    pub fn retain_terms(&self, keep: impl Fn(&Ty) -> bool) -> Ty {
        let terms: Vec<Ty> = self.terms_vec().into_iter().filter(|t| keep(t)).collect();
        let mut out = Ty::from_terms(terms);
        out.mu = self.mu;
        out.normalise_mu()
    }

    /// The type a positional read yields over a UNION, term by term: a positional
    /// shape answers its position (or `nil` past its end), a uniform sequence its
    /// element with `nil` beside it (the read may run off the end), `nil` reads `nil`.
    /// `index` is the literal position, or `None` for `last`. `None` when some term is
    /// not a collection at all.
    pub fn element_at_over_union(&self, index: Option<usize>) -> Option<Ty> {
        let mut out = Ty::NEVER;
        for term in self.terms_vec() {
            if term.tags == NIL_BIT {
                out = out.union(Ty::of(Tag::Nil));
                continue;
            }
            let read = match term.positional_elems() {
                Some(elems) => match index {
                    Some(i) => elems.get(i).cloned().unwrap_or(Ty::of(Tag::Nil)),
                    None => elems.last().cloned().unwrap_or(Ty::of(Tag::Nil)),
                },
                None => term.elem_ty()?.union(Ty::of(Tag::Nil)),
            };
            out = out.union(read);
        }
        Some(out)
    }

    /// The element-type refinement, if this sequence type carries one (or can
    /// be derived from one) — the bridge the checker reads to flow `(first
    /// xs)` / `(nth xs i)` to the element type. A tuple has no plain `elem`,
    /// but the union of its per-position types is exactly as sound a bound
    /// (every element of a `tuple<int, string>` is an `int | string`), so
    /// derive that when `elem` itself is absent (ADR-128) — this is the
    /// single choke point every `elem_ty` consumer already goes through, so
    /// `first`/`nth`/`rest`/etc. all pick up a tuple-typed vector for free.
    /// Owned (not `&Ty`) because the tuple case synthesizes a fresh value;
    /// every existing caller already immediately `.cloned()`s the borrowed
    /// case anyway.
    pub fn elem_ty(&self) -> Option<Ty> {
        if self.mu {
            return self.unroll().elem_ty();
        }
        let this = self.single()?;
        // A refinement the type CARRIES describes every sequence member it was put on,
        // and is the answer wherever it is present.
        if let Some(elem) = this.elem.as_deref() {
            return Some(elem.clone());
        }
        // Otherwise the element type can still be DERIVED from the kind — for the three
        // collections below. Each derivation speaks for the whole term, so each requires
        // the term to admit exactly one collection: when it admits a second, that
        // member's elements are not covered and the term's are unknown. Answering for one
        // member is the accepting direction of unsound — it produced a checker false
        // positive, `(tuple 0) ∪ pair` (a fold whose init is `[0]` and whose step conses
        // strings) answering `0` for `first`, so `(takes-str (first …))` warned "expects
        // string, got 0" on a value that was the string "t" at runtime.
        match this.tags & (SEQ_BITS | MAP_BIT | BYTES_BIT) {
            // A tuple has no plain `elem`, but its per-position types taken together are
            // exactly as sound a bound (every element of a `tuple<int, string>` is an
            // `int | string`). Positions only ever describe a VECTOR.
            VECTOR_BIT => this
                .tuple
                .as_ref()
                .map(|elems| elems.iter().cloned().fold(Ty::NEVER, |acc, t| acc.union(t))),
            // …and a list shape's, for the pair member.
            PAIR_BIT => this
                .list_shape
                .as_ref()
                .map(|elems| elems.iter().cloned().fold(Ty::NEVER, |acc, t| acc.union(t))),
            // `bytes` is a sequence of octets: its element type is fixed by the kind, so
            // it needs no refinement to carry one — and an octet is `int[0..255]`, which
            // is what makes `(nth codes (math/rem b 16))` an index the checker can bound.
            BYTES_BIT => Some(Ty::int_in(Range::new(Some(0), Some(255)))),
            // A map walks as its `[key value]` entries — a two-element VECTOR, verified
            // against the runtime rather than assumed. What is known of the key and the
            // value comes from whichever refinement the map carries: `map_kv` states them
            // directly, and a CLOSED record shape states them exactly (its keys are the
            // declared ones and nothing else — an OPEN shape may carry keys nothing
            // declares, so it declines). A map the checker knows nothing about declines
            // too — see the Seqable note below.
            MAP_BIT => {
                let (key, value) = if let Some((k, v)) = this.map_kv.as_deref() {
                    (k.clone(), v.clone())
                } else if let (Some(fields), Some(false)) =
                    (self.record_fields(), self.record_is_open())
                {
                    fields.iter().fold(
                        (Ty::NEVER, Ty::NEVER),
                        |(keys, values), (name, (ty, _optional))| {
                            (keys.union(Ty::keyword_lit(*name)), values.union(ty.clone()))
                        },
                    )
                } else {
                    // Neither: decline. An OPEN shape says nothing about the keys a
                    // value carries, and a NOMINAL record is open — which is the case
                    // that must not be answered, because a record may implement Seqable
                    // and then it walks as whatever that impl yields rather than as
                    // entries. `tests/queue_test.blsp` maps over a queue, and the
                    // "an entry is at least a pair of unknowns" fallback this replaces
                    // typed its elements as `(tuple any, any)` and flagged the callback.
                    return None;
                };
                Some(Ty::tuple_of(vec![key, value]))
            }
            _ => None,
        }
    }

    /// The type of a concrete value — the bridge from a runtime value to its type.
    /// A keyword/int/bool becomes its **literal singleton** (`:foo`/`5`/`true`, not
    /// the whole `keyword`/`int`/`bool` tag), so a literal in code is checked
    /// precisely against an enumerated sig (`5` vs `(or 5 6 7)`). Ints and bools
    /// were once left flat here to avoid the message-wording churn a singleton
    /// causes ("got int" → "got 5"); that churn was accepted and the singletons
    /// shipped in B0 (gating — see `docs/type-int-literals.md`). Strings need the
    /// heap to read their bytes, so `of_value` (heap-free) leaves a string flat and
    /// `expr_ty` builds the `str_lit` where it has the heap. Bignums stay flat `int`.
    pub fn of_value(v: Value) -> Ty {
        match v {
            // Literal singletons (B0 — literal-singleton precision): a literal's
            // *exact* type is the singleton, e.g. `5 : {5}` (a subtype of `int`),
            // so a literal argument checks precisely against a literal-set param
            // (`5` vs `(or 5 6 7)`). String singletons need the heap to read the
            // bytes, so `of_value` (heap-free) leaves a string flat; `expr_ty`
            // builds the `str_lit` where it has the heap.
            Value::Keyword(s) => Ty::keyword_lit(s),
            Value::Int(n) => Ty::int_lit(n),
            Value::Bool(b) => Ty::bool_lit(b),
            _ => Ty::of(value::tag(v)),
        }
    }

    /// The type asserted when the named type-predicate holds — the bridge from a
    /// guard `(pred x)` to a refinement of `x`'s type (occurrence typing, step 4):
    /// in the *then* branch `x` narrows to `T ∩ tested_by(pred)`, in the *else*
    /// branch to `T ∩ ¬tested_by(pred)`. `None` for predicates that don't pin a
    /// tag (`empty?`, `zero?`, …) and for unknown names. Spellings match the
    /// `int?`/`string?`/… builtins and the prelude's `number?`/`list?`.
    ///
    /// Keyed by `&str` for now; the Step 4 pass holds interned `Symbol`s, so this
    /// may move to a `Symbol`-keyed lookup if it proves hot.
    pub fn tested_by(predicate: &str) -> Option<Ty> {
        Some(match predicate {
            "nil?" => Ty::of(Tag::Nil),
            "bool?" => Ty::of(Tag::Bool),
            "int?" => Ty::of(Tag::Int),
            "float?" => Ty::of(Tag::Float),
            "decimal?" => Ty::of(Tag::Decimal),
            "ratio?" => Ty::of(Tag::Ratio),
            "symbol?" => Ty::of(Tag::Sym),
            "keyword?" => Ty::of(Tag::Keyword),
            "string?" => Ty::of(Tag::Str),
            "bytes?" => Ty::of(Tag::Bytes),
            "pair?" => Ty::of(Tag::Pair),
            "vector?" => Ty::of(Tag::Vector),
            "map?" => Ty::of(Tag::Map),
            "set?" => Ty::of(Tag::Set),
            "ref?" => Ty::of(Tag::Ref),
            "pid?" => Ty::of(Tag::Pid),
            "rope?" => Ty::of(Tag::Rope),
            "socket?" => Ty::of(Tag::Socket),
            "subprocess?" => Ty::of(Tag::Subprocess),
            "table?" => Ty::of(Tag::Table),
            // ADR-310 appended `Failure` as a tag; a predicate over it narrows
            // like any other, so `(if (failure? n) 0 n)` types the else branch
            // without the failure in it.
            "failure?" => Ty::of(Tag::Failure),
            // `fn?` holds for both Brood closures and Rust builtins.
            "fn?" => Ty::of(Tag::Fn).union(Ty::of(Tag::Native)),
            "number?" => Ty::NUMBER,
            "list?" => Ty::LIST,
            _ => return None,
        })
    }

    /// The type a truthy `(pred x)` PROVES of `x` when the predicate is not biconditional:
    /// every value it accepts has this type, but so do values it rejects, so only the
    /// then-branch may narrow. `record?` holds for a `defrecord` value — a map — and fails
    /// for a plain map, so a guard on it narrows `number | usd` to `usd` and its else
    /// branch to nothing; read as `tested_by` it would have `(seq/reject xs record?)` drop
    /// every map. `None` for a name [`tested_by`](Ty::tested_by) answers, or no predicate.
    pub fn implied_by(predicate: &str) -> Option<Ty> {
        Some(match predicate {
            "record?" => Ty::of(Tag::Map),
            _ => return None,
        })
    }

    /// `self ∪ other` — values in either. A refinement survives only where it's
    /// unambiguous: if just one side contributes the relevant members (functions
    /// for `arrow`, sequences for `elem`), that side's refinement carries; if both
    /// do, it survives only when they agree (the union of two distinct
    /// arrows/element-types isn't a single one → widen to "any"). Widening is
    /// sound: a union is a supertype anyway.
    /// Bounded node count of this `Ty`'s refinement tree, walked as a tree and stopping
    /// once it reaches `lim` (so a shared `Arc` DAG that is exponential as a tree can't
    /// make the count itself blow up). Used only by [`bounded`](Ty::bounded).
    fn node_count(&self, lim: usize) -> usize {
        let mut n = 1usize;
        if n >= lim {
            return n;
        }
        if let Some(e) = &self.elem {
            n += e.node_count(lim - n);
            if n >= lim {
                return n;
            }
        }
        if let Some(kv) = &self.map_kv {
            n += kv.0.node_count(lim - n);
            if n >= lim {
                return n;
            }
            n += kv.1.node_count(lim - n);
            if n >= lim {
                return n;
            }
        }
        if let Some(f) = &self.fields {
            n += f.rest.node_count(lim - n);
            if n >= lim {
                return n;
            }
            for (t, _) in f.fields.values() {
                n += t.node_count(lim - n);
                if n >= lim {
                    return n;
                }
            }
        }
        for ts in [&self.tuple, &self.list_shape].into_iter().flatten() {
            for t in ts.iter() {
                n += t.node_count(lim - n);
                if n >= lim {
                    return n;
                }
            }
        }
        n
    }

    /// This type with every structural refinement (elements, key/value, fields, tuple
    /// positions) nested more than `depth` levels down DROPPED to its tags — the literal
    /// sets, the subtraction and the alternatives are kept. `widened_below(0)` is the flat
    /// tag set with its literals.
    ///
    /// The **widening operator** a fixpoint over types needs (Pass 2.9, ADR-341): a JSON
    /// value contains vectors of JSON values, and with no recursive types the inferred
    /// element refinement nests one level deeper every round — `vector<… | vector<… |
    /// vector<…>>>` — until [`MAX_TY_NODES`] flattens it, after which it regrows, and the
    /// sequence never repeats. Applied to what is still moving after a few rounds, it makes
    /// the ascent stationary: what a round computes from inputs of depth `d` has depth at most
    /// `d + 1`, and widening it back to `d` gives the previous round's value. Sound — a
    /// dropped refinement is an over-approximation, like every widening here.
    pub fn widened_below(&self, depth: usize) -> Ty {
        // A recursive type is finite already — its self-references are the depth — and
        // cutting into its body would only strand them.
        if self.mu {
            return self.clone();
        }
        let mut out = self.clone();
        if depth == 0 {
            out.arrow = None;
            out.overload = None;
            out.elem = None;
            out.map_kv = None;
            out.fields = None;
            out.tuple = None;
            out.list_shape = None;
        } else {
            let below = |t: &Ty| t.widened_below(depth - 1);
            out.elem = self.elem.as_ref().map(|e| Arc::new(below(e)));
            out.map_kv = self
                .map_kv
                .as_ref()
                .map(|kv| Arc::new((below(&kv.0), below(&kv.1))));
            out.fields = self.fields.as_ref().map(|f| {
                Arc::new(RecordShape {
                    fields: f
                        .fields
                        .iter()
                        .map(|(k, (t, req))| (*k, (below(t), *req)))
                        .collect(),
                    rest: below(&f.rest),
                })
            });
            out.tuple = self
                .tuple
                .as_ref()
                .map(|ts| Arc::new(ts.iter().map(below).collect()));
            out.list_shape = self
                .list_shape
                .as_ref()
                .map(|ts| Arc::new(ts.iter().map(below).collect()));
            out.arrow = self
                .arrow
                .as_ref()
                .map(|s| Arc::new(widen_sig(s, depth - 1)));
            out.overload = self
                .overload
                .as_ref()
                .map(|ss| Arc::new(ss.iter().map(|s| widen_sig(s, depth - 1)).collect()));
        }
        // The alternatives and the subtraction sit at THIS level.
        out.alts = self
            .alts
            .as_ref()
            .map(|a| Arc::new(a.iter().map(|t| t.widened_below(depth)).collect()));
        out.neg = self
            .neg
            .as_ref()
            .map(|n| Arc::new(n.iter().map(|t| t.widened_below(depth)).collect()));
        out
    }
    /// If this type's refinement tree exceeds [`MAX_TY_NODES`] nodes, widen it to the flat
    /// tag set (dropping the structural refinements; the bounded literal sets are kept).
    /// Sound — widening over-approximates, never manufacturing a false positive — and it
    /// bounds every `Ty` to a fixed size so `union`/`==`/`Hash`/`is_subtype` stay linear.
    fn bounded(self) -> Ty {
        // fast path: no structural refinement means nothing to measure or drop.
        if self.elem.is_none()
            && self.map_kv.is_none()
            && self.fields.is_none()
            && self.tuple.is_none()
            && self.list_shape.is_none()
        {
            return self;
        }
        if self.node_count(MAX_TY_NODES + 1) <= MAX_TY_NODES {
            return self;
        }
        // A positional shape over the cap — a long quoted list, a wide literal vector —
        // degrades to its ELEMENT union first (each position's union, widened to its
        // tags), which is the bound the shape stood for; only past that does everything
        // go. Without this step a hundred-element `'(1 2 …)` read as a bare `pair`, where
        // the plain element rule had kept `list<int>`.
        if self.tuple.is_some() || self.list_shape.is_some() {
            // First, the shape with FLAT positions: a `[model idx]` whose model record is
            // deep is still a pair a destructuring reads as `map` and `int` — dropping to
            // the element union made it `int | map` at both positions (ADR-350). Only a
            // shape too wide for even that goes to the element union.
            let flat = |ts: &Option<Arc<Vec<Ty>>>| {
                ts.as_ref()
                    .map(|ts| Arc::new(ts.iter().map(|t| t.widened_below(0)).collect::<Vec<Ty>>()))
            };
            let shaped = Ty {
                tuple: flat(&self.tuple),
                list_shape: flat(&self.list_shape),
                elem: self.elem.as_ref().map(|e| Arc::new(e.widened_below(0))),
                ..self.clone()
            };
            if shaped.node_count(MAX_TY_NODES + 1) <= MAX_TY_NODES {
                return shaped;
            }
            let positions = self
                .tuple
                .iter()
                .chain(self.list_shape.iter())
                .flat_map(|ts| ts.iter())
                .fold(Ty::NEVER, |acc, t| acc.union(t.widened_below(0)));
            let elem = match &self.elem {
                Some(e) => e.as_ref().clone().union(positions),
                None => positions,
            };
            let degraded = Ty {
                elem: Some(Arc::new(elem)),
                tuple: None,
                list_shape: None,
                ..self
            };
            if degraded.node_count(MAX_TY_NODES + 1) <= MAX_TY_NODES {
                return degraded;
            }
            return degraded.bounded();
        }
        Ty {
            arrow: None,
            overload: None,
            elem: None,
            map_kv: None,
            fields: None,
            tuple: None,
            list_shape: None,
            ..self
        }
    }

    fn union_term(self, other: Ty) -> Ty {
        let tags = self.tags | other.tags;
        let arrow = merge_union(
            self.tags & FN_BITS != 0,
            &self.arrow,
            other.tags & FN_BITS != 0,
            &other.arrow,
        );
        // Same "widen unless identical" rule as every other refinement — an
        // overload set is just another `Option<Arc<T: PartialEq>>` as far as
        // `merge_union` is concerned.
        let overload = merge_union(
            self.tags & FN_BITS != 0,
            &self.overload,
            other.tags & FN_BITS != 0,
            &other.overload,
        );
        // The element slot has an EXACT merge beyond "identical or one-sided": when one
        // side's elements sit inside the other's and so do its sequence kinds, the union
        // of the two sequence parts is the wider side's — see [`elem_union_exact`].
        // `merge_is_exact` has already agreed, so `None` here is the plain widening.
        let elem = elem_union_exact(&self, &other).unwrap_or_else(|| {
            merge_union(
                self.tags & SEQ_BITS != 0,
                &self.elem,
                other.tags & SEQ_BITS != 0,
                &other.elem,
            )
        });
        let map_kv = merge_union(
            self.tags & MAP_BIT != 0,
            &self.map_kv,
            other.tags & MAP_BIT != 0,
            &other.map_kv,
        );
        let fields = merge_union(
            self.tags & MAP_BIT != 0,
            &self.fields,
            other.tags & MAP_BIT != 0,
            &other.fields,
        );
        // Two tuple shapes of one arity: exact when they differ in one position
        // (`A×B ∪ C×B = (A∪C)×B`), and otherwise the per-position union — a WIDENING that
        // keeps the arity and every other position, where the plain rule dropped the
        // shape to a bare `vector` (see [`tuple_union`]).
        let tuple = positional_union(&self.tuple, self.tags, &other.tuple, other.tags, VECTOR_BIT)
            .unwrap_or_else(|| {
                merge_union(
                    self.tags & VECTOR_BIT != 0,
                    &self.tuple,
                    other.tags & VECTOR_BIT != 0,
                    &other.tuple,
                )
            });
        let list_shape = positional_union(
            &self.list_shape,
            self.tags,
            &other.list_shape,
            other.tags,
            PAIR_BIT,
        )
        .unwrap_or_else(|| {
            merge_union(
                self.tags & PAIR_BIT != 0,
                &self.list_shape,
                other.tags & PAIR_BIT != 0,
                &other.list_shape,
            )
        });
        // Literal sets union *exactly* (not widen) — `:a ∪ :b = {a,b}` — unless a
        // side has that member *open* (tag present, no set), which contributes
        // every value of the tag (`:a ∪ keyword = keyword`). Each tag is
        // independent, so `(or :ok 5)` carries both a keyword- and an int-literal
        // set with no special-casing.
        let lit = merge_union_lit_set(KEYWORD_BIT, self.tags, &self.lit, other.tags, &other.lit);
        let lit_int = merge_union_lit_set(
            INT_BIT,
            self.tags,
            &self.lit_int,
            other.tags,
            &other.lit_int,
        );
        let lit_bool = canon_lit_bool(merge_union_lit_set(
            BOOL_BIT,
            self.tags,
            &self.lit_bool,
            other.tags,
            &other.lit_bool,
        ));
        let lit_str = merge_union_lit_set(
            STR_BIT,
            self.tags,
            &self.lit_str,
            other.tags,
            &other.lit_str,
        );
        // Intervals union by HULL — a widening (`[0,0] ∪ [5,∞)` admits 3), which is what
        // keeps an index's type one term; a positive literal set makes the interval
        // redundant (canonical form). One-sided where only one side has the member.
        let int_range = merge_ranges(
            self.tags & INT_BIT != 0,
            self.int_range_eff(),
            other.tags & INT_BIT != 0,
            other.int_range_eff(),
        );
        let int_range = canon_int_range(&lit_int, int_range);
        let len = canon_len(
            tags,
            merge_ranges(
                self.tags & COUNT_BITS != 0,
                self.len_eff(),
                other.tags & COUNT_BITS != 0,
                other.len_eff(),
            ),
        );
        Ty {
            tags,
            int_range,
            len,
            // A merged union subtracts nothing: `merge_is_exact` refuses to merge two
            // terms when either carries a subtraction, so this arm only ever runs on
            // positives (ADR-288).
            neg: None,
            // Two self-references merge to one; `merge_is_exact` keeps a self-reference
            // apart from anything else, and the past-the-cap widening that reaches here
            // regardless reads the merge as `any` (the flag goes, the tags stay).
            rec_ref: self.rec_ref && other.rec_ref,
            mu: false,
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
            alts: None,
        }
        // Bound the result's size so a fixpoint that unions nested branch results (a
        // recursive value-builder) can't grow the type without limit — every subsequent
        // `union`/`is_subtype`/`==` input then stays within the cap (KI-13).
        .normalise_len()
        .bounded()
    }

    /// `self ∩ other` — values in both. When the relevant bit survives and one
    /// side is unrefined ("any"), the other side's refinement is the narrower —
    /// keep it; two distinct known refinements can't be one → widen. (Used by
    /// guard narrowing `T ∩ tested_by(pred)`, where `tested_by` is flat, so a
    /// refined `T` keeps its refinement through the narrow.)
    fn intersect_term(self, other: Ty) -> Ty {
        // A dangling self-reference is an unknown set `S`: `S ∩ T ⊆ T`, so `T` is the
        // sound answer (and `S ∩ S` is `S`).
        if self.rec_ref && other.rec_ref {
            return self;
        }
        if self.rec_ref {
            return other;
        }
        if other.rec_ref {
            return self;
        }
        let mut tags = self.tags & other.tags;
        let (arrow, overload) = if tags & FN_BITS != 0 {
            intersect_arrows(&self, &other)
        } else {
            (None, None)
        };
        // Element types intersect **exactly**: `vector<int> ∩ vector<string>` is
        // `vector<never>` — the empty vector, which really does inhabit both — not
        // `vector`, which the generic widening merge produced and which is not even a
        // subtype of either operand. (An intersection must be a lower bound; that
        // property is now asserted over the whole corpus.)
        let elem = if tags & SEQ_BITS != 0 {
            match (&self.elem, &other.elem) {
                (Some(a), Some(b)) => {
                    Some(Arc::new(a.as_ref().clone().intersect(b.as_ref().clone())))
                }
                (a, b) => merge_intersect(a, b),
            }
        } else {
            None
        };
        let map_kv = if tags & MAP_BIT != 0 {
            merge_intersect(&self.map_kv, &other.map_kv)
        } else {
            None
        };
        // Record shapes intersect **exactly** (see `intersect_records`) rather than
        // widening to `None` the way the generic refinement merge does — this is the
        // narrowing a type guard performs, and dropping it would lose the fact the
        // guard just established. A shape pair no value satisfies clears the `map` bit.
        let fields = if tags & MAP_BIT != 0 {
            match (&self.fields, &other.fields) {
                (Some(a), Some(b)) => match intersect_records(a, b) {
                    Some(shape) => Some(Arc::new(shape)),
                    None => {
                        tags &= !MAP_BIT;
                        None
                    }
                },
                (a, b) => merge_intersect(a, b),
            }
        } else {
            None
        };
        // Positional shapes intersect **exactly**, and unlike an element type a tuple's
        // arity *is* its shape: two different arities, or one position no value
        // satisfies, means no vector is both — so the `vector` bit goes, rather than
        // widening to plain `vector` (which `is_disjoint` correctly calls disjoint,
        // leaving the two answers contradicting each other).
        let tuple = if tags & VECTOR_BIT != 0 {
            match (&self.tuple, &other.tuple) {
                (Some(a), Some(b)) => match intersect_tuples(a, b) {
                    Some(elems) => Some(Arc::new(elems)),
                    None => {
                        tags &= !VECTOR_BIT;
                        None
                    }
                },
                (a, b) => merge_intersect(a, b),
            }
        } else {
            None
        };
        // The same for a list shape on the pair member.
        let list_shape = if tags & PAIR_BIT != 0 {
            match (&self.list_shape, &other.list_shape) {
                (Some(a), Some(b)) => match intersect_tuples(a, b) {
                    Some(elems) => Some(Arc::new(elems)),
                    None => {
                        tags &= !PAIR_BIT;
                        None
                    }
                },
                (a, b) => merge_intersect(a, b),
            }
        } else {
            None
        };
        // Literal sets intersect; an empty result means no value of the tag
        // qualifies, so clear that tag bit. An *open* side (tag, no set) intersects
        // to the other side's set (the narrower). Each tag is independent.
        let lit = if tags & KEYWORD_BIT != 0 {
            let (s, keep) = intersect_lit_set(&self.lit, &other.lit);
            if !keep {
                tags &= !KEYWORD_BIT;
            }
            s
        } else {
            None
        };
        let lit_int = if tags & INT_BIT != 0 {
            let (s, keep) = intersect_lit_set(&self.lit_int, &other.lit_int);
            if !keep {
                tags &= !INT_BIT;
            }
            s
        } else {
            None
        };
        // The int intervals MEET, and a positive literal set is filtered by the meet —
        // `{3, 5, 7} ∩ [4, 10]` is `{5, 7}` — an empty result dropping the tag.
        let (lit_int, int_range) = if tags & INT_BIT != 0 {
            match Range::meet(self.int_range_eff(), other.int_range_eff()) {
                None => {
                    tags &= !INT_BIT;
                    (None, None)
                }
                Some(r) => {
                    let lit_int = filter_lit_int(lit_int, r);
                    if matches!(lit_int.as_deref(), Some(LitSet::In(s)) if s.is_empty()) {
                        tags &= !INT_BIT;
                        (None, None)
                    } else {
                        let range = canon_int_range(&lit_int, Some(r));
                        (lit_int, range)
                    }
                }
            }
        } else {
            (None, None)
        };
        // The lengths meet too; no length at all means no countable member survives,
        // and a length of exactly 0 is no `pair` (a list of length 0 is `nil`).
        let len = if tags & COUNT_BITS != 0 {
            match Range::meet(self.len_eff(), other.len_eff()) {
                None => {
                    tags &= !COUNT_BITS;
                    None
                }
                Some(r) => {
                    if r.hi == Some(0) {
                        tags &= !PAIR_BIT;
                    }
                    canon_len(tags, Some(r))
                }
            }
        } else {
            None
        };
        let lit_bool = if tags & BOOL_BIT != 0 {
            let (s, keep) = intersect_lit_set(&self.lit_bool, &other.lit_bool);
            if !keep {
                tags &= !BOOL_BIT;
            }
            canon_lit_bool(s)
        } else {
            None
        };
        let lit_str = if tags & STR_BIT != 0 {
            let (s, keep) = intersect_lit_set(&self.lit_str, &other.lit_str);
            if !keep {
                tags &= !STR_BIT;
            }
            s
        } else {
            None
        };
        // `(Pa ∖ Na) ∩ (Pb ∖ Nb) = (Pa ∩ Pb) ∖ (Na ∪ Nb)` — the positives meet as they
        // always did and the subtractions simply accumulate (ADR-288).
        let neg = union_negs(&self.neg, &other.neg);
        Ty {
            tags,
            neg,
            rec_ref: false,
            mu: false,
            int_range,
            len,
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
            alts: None,
        }
        .normalise_len()
        .bounded()
    }

    /// `¬self` — every value *not* in `self`, as a **sound over-approximation**:
    /// the result is always a *superset* of the true complement, never a subset.
    /// Exact for a flat type. For a *refined* type it can't be exact — the
    /// complement of `vector<int>` is "non-vectors **plus** vectors holding a
    /// non-int", which this flat lattice can't name — so we widen: drop the
    /// refinement *and keep the refined tag in the result*, because some of that
    /// tag's inhabitants escape the refinement and so live in the complement.
    /// Keeping the tag is what makes the result a superset; the earlier "drop the
    /// tag too" produced a *subset* — unsound, it could manufacture a false
    /// [`is_disjoint`](Ty::is_disjoint). Widening a complement can only ever
    /// *suppress* a disjointness warning, never raise a false one
    /// (advisory-soundness). Consequence: `a ∩ ¬a = ⊥` and double-negation are
    /// exact only for **flat** `a` (which is all the laws tests sample, and all
    /// the checker ever negates — `tested_by`/`%eq` results are flat).
    fn negate_term(self) -> Ty {
        // A dangling self-reference is an unknown set, whose complement is unknown.
        if self.rec_ref {
            return Ty::ANY;
        }
        // Negating a term that already SUBTRACTS something: `¬(P ∖ N) = ¬P ∪ (P ∩ ⋃N)`.
        // Without this, `¬¬(vector int)` read the subtraction-free path, complemented
        // `UNIVERSE` to nothing and answered `never` — double negation destroying the type
        // rather than restoring it. The recursion terminates because `positive()` carries
        // no subtraction of its own.
        if let Some(negs) = &self.neg {
            let positive = self.positive();
            let mut out = positive.clone().negate_term();
            for n in negs.iter() {
                out = out.union(positive.clone().intersect(n.clone()));
            }
            return out;
        }
        let mut tags = !self.tags & UNIVERSE;
        // A refinement means `self` omits some values of its refined tag(s);
        // those omitted values are in the complement, so the tag must survive.
        if self.arrow.is_some() || self.overload.is_some() {
            tags |= self.tags & FN_BITS;
        }
        if self.elem.is_some() {
            tags |= self.tags & SEQ_BITS;
        }
        if self.map_kv.is_some() || self.fields.is_some() {
            tags |= self.tags & MAP_BIT;
        }
        if self.tuple.is_some() {
            tags |= self.tags & VECTOR_BIT;
        }
        if self.list_shape.is_some() {
            tags |= self.tags & PAIR_BIT;
        }
        // A literal set omits the *other* values of its tag, which are in the
        // complement — so the tag survives (widened to "any" of that tag). Each
        // literal kind is an independent tag/field.
        for (present, bit) in [
            (self.lit.is_some(), KEYWORD_BIT),
            (self.lit_int.is_some(), INT_BIT),
            (self.lit_bool.is_some(), BOOL_BIT),
            (self.lit_str.is_some(), STR_BIT),
        ] {
            if present {
                tags |= bit;
            }
        }
        // …except **bool**, whose domain is finite. `¬{false}` within the bool members
        // is exactly `{true}`, so this one complement is representable rather than
        // widened — and it is the one that matters: *truthy* is `¬(nil ∪ false)`, the
        // type every `(if x …)` narrows to. Widened, that lands on `not nil`, which is
        // a sound necessary condition but not invertible (a false test does not imply
        // `nil`), and the truthiness guard has to be one-sided as a result. Exact, it
        // is biconditional.
        //
        // The other three literal kinds have infinite domains — `¬5` within the ints is
        // not a set this lattice can hold — so they keep widening. That asymmetry is
        // the whole of the "negative atoms" gap, narrowed to where it is unavoidable.
        // Each literal kind's complement *within its tag* — `In(A) ↔ Out(A)`, exactly.
        // This is the piece that makes an equality test narrow its else branch:
        // `(or :ok :err) ∩ ¬:ok` is `:err`, where before `¬:ok` was `any` and the
        // narrowing was lost. The tag bits are already set above (a literal set omits
        // only *some* of its tag's values, so the tag survives the complement).
        // **A structural refinement cannot be complemented in place.** Flipping the tag
        // keeps every vector, so `¬(vector int)` widened to `any` and
        // `(vector int) ∩ ¬(vector int)` came out `(vector int)` instead of `never` — a
        // guard's else branch learning nothing. Say it exactly instead: everything, minus
        // this term (ADR-288). The tag and literal complements below stay in place, because
        // those ARE expressible and the flat `(not string)` rendering is worth keeping.
        if self.arrow.is_some()
            || self.overload.is_some()
            || self.elem.is_some()
            || self.tuple.is_some()
            || self.list_shape.is_some()
            || self.map_kv.is_some()
            || self.fields.is_some()
            || self.int_range.is_some()
            || self.len.is_some()
        {
            return Ty {
                neg: Some(Arc::new(vec![self.positive()])),
                ..Ty::flat(UNIVERSE)
            };
        }
        let mut out = Ty::flat(tags);
        out.lit = self.lit.as_deref().map(|set| Arc::new(set.complement()));
        out.lit_int = self
            .lit_int
            .as_deref()
            .map(|set| Arc::new(set.complement()));
        out.lit_str = self
            .lit_str
            .as_deref()
            .map(|set| Arc::new(set.complement()));
        // …except **bool**, whose domain is finite: its complement is normalised back to
        // a positive set, and if that leaves nothing the tag itself drops. Holding bool
        // positively is what keeps `LitSet::Out`'s "infinite complement" assumption true
        // for every set that actually exists (see [`LitSet`]).
        if let Some(LitSet::In(set)) = self.lit_bool.as_deref() {
            let complement: BTreeSet<bool> = [true, false]
                .into_iter()
                .filter(|b| !set.contains(b))
                .collect();
            if complement.is_empty() {
                out.tags &= !BOOL_BIT; // `¬{true, false}` admits no bool at all
            } else {
                out.lit_bool = canon_lit_bool(Some(Arc::new(LitSet::In(complement))));
            }
        }
        out
    }

    // ---- the union of terms (ADR-262) ----
    //
    // Everything above operates on ONE term. These five are the public set
    // operations, and they quantify over a type's terms — which is what makes a
    // union of two structured types (`(or (tuple int) (tuple string))`) survive
    // being written down instead of widening to bare `vector`.

    /// This type's terms: itself when single (the common case), else the head term
    /// followed by its alternatives. Every term is alts-free.
    fn terms_vec(&self) -> Vec<Ty> {
        match &self.alts {
            None => vec![self.clone()],
            Some(rest) => {
                let mut out = Vec::with_capacity(rest.len() + 1);
                out.push(self.head_term());
                out.extend(rest.iter().cloned());
                out
            }
        }
    }

    /// This type's terms with ITS OWN binder stripped — the body of a recursive type
    /// (open: the references inside are this type's), where [`Ty::terms_vec`] keeps a
    /// single recursive type whole so that, pushed into a union, it stays a closed
    /// alternative. An alternative that is itself a binder stays closed here too.
    fn body_terms(&self) -> Vec<Ty> {
        let mut terms = self.terms_vec();
        if let Some(first) = terms.first_mut() {
            first.mu = false;
        }
        terms
    }

    /// This type without its alternatives — the head term alone.
    fn head_term(&self) -> Ty {
        Ty {
            alts: None,
            mu: false,
            ..self.clone()
        }
    }

    /// Build a type from a list of terms, restoring every invariant: `never` terms
    /// dropped, subsumed terms absorbed, exactly-mergeable terms merged, and the
    /// count capped at [`MAX_TY_TERMS`] by collapsing the remainder with the widening
    /// merge (which is what a union always did, so the cap can only lose precision,
    /// never soundness).
    fn from_terms(mut terms: Vec<Ty>) -> Ty {
        terms.retain(|t| !t.is_never());
        if terms.is_empty() {
            return Ty::NEVER;
        }
        // Merge and absorb until neither applies. Quadratic in the term count, which
        // is bounded by the cap below — a handful at most.
        let mut merged: Vec<Ty> = Vec::with_capacity(terms.len());
        // Containment here must see SUBTRACTIONS (ADR-288). `is_subtype_term` compares
        // positive slots only, so it reads `any` and `¬vector<int>` as the same shape —
        // both `UNIVERSE` with no refinement — and absorbed `any` into the narrower one,
        // making `A ∪ B` depend on the order of A and B. `term_covered` asks the question
        // the subtraction actually poses.
        let contains = |outer: &Ty, inner: &Ty| term_covered(inner, std::slice::from_ref(outer));
        'next: for t in terms {
            for existing in merged.iter_mut() {
                if contains(existing, &t) {
                    continue 'next; // absorbed
                }
                if contains(&t, existing) {
                    *existing = t;
                    continue 'next;
                }
                if merge_is_exact(existing, &t) {
                    *existing = existing.clone().union_term(t);
                    continue 'next;
                }
            }
            merged.push(t);
        }
        // Absorption is not one-pass: a term that *replaces* an earlier one (or a merge
        // that widens one) can subsume terms already accepted, and those were compared
        // against the old value. Without this sweep `any ∪ (A | B)` kept `B` as a
        // redundant alternative while `(A | B) ∪ any` did not — the same set, unequal,
        // which breaks every memo keyed on a `Ty`.
        let mut i = 0;
        while i < merged.len() {
            let absorbed = merged
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && contains(other, &merged[i]));
            if absorbed {
                merged.remove(i);
            } else {
                i += 1;
            }
        }
        // Past the cap, the widening merge — sound, less precise. TAG-AWARE first: two
        // positional shapes sharing their tags and not told apart by a keyword (the four
        // `[:error status why …]` arms of a result) merge before anything hulls the
        // `[:ok …]` arm into them; only when no such pair is left do the last two go,
        // whatever they are. Blind, the result with five error arms lost its `:ok` shape
        // (`(tuple :ok (or 502 map) any)`), and a `match` on the tag could then read
        // nothing from it — and raising the cap instead (eight) took the match compiler's
        // own file from 0.6 s to past five minutes, since the relations quantify over
        // subsets of the alternatives.
        while merged.len() > MAX_TY_TERMS {
            let pair = (0..merged.len())
                .flat_map(|i| ((i + 1)..merged.len()).map(move |j| (i, j)))
                .find(|&(i, j)| {
                    let (a, b) = (&merged[i], &merged[j]);
                    a.tags & !NIL_BIT == b.tags & !NIL_BIT
                        && a.fields.is_none()
                        && b.fields.is_none()
                        && !tagged_apart(a, b)
                });
            let (i, j) = pair.unwrap_or((merged.len() - 2, merged.len() - 1));
            let second = merged.remove(j);
            let first = merged.remove(i);
            merged.push(first.union_term(second));
        }
        let mut head = merged.remove(0);
        if !merged.is_empty() {
            head.alts = Some(Arc::new(merged));
        }
        head
    }

    /// `self ∪ other` — every value in either.
    pub fn union(self, other: Ty) -> Ty {
        // A μ on either side stays a μ of the union: its references then name the
        // union, a SUPERSET of what they named — sound, and exact when only one side is
        // recursive (`μX.G[X] ∪ T = μX.(G[X] ∪ T)` for any `T` the references do not
        // reach). Two binders conflate into one, an over-approximation of both.
        let mu = self.mu || other.mu;
        let out = self.union_positive(other);
        if mu {
            Ty { mu: true, ..out }.normalise_mu()
        } else {
            out
        }
    }

    fn union_positive(self, other: Ty) -> Ty {
        // The single-term fast path is the old behaviour verbatim, including its
        // widening merge, so nothing that already had one representable term pays for
        // this or renders differently.
        if self.alts.is_none() && other.alts.is_none() && merge_is_exact(&self, &other) {
            return self.union_term(other);
        }
        let mut terms = self.terms_vec();
        terms.extend(other.terms_vec());
        Ty::from_terms(terms)
    }

    /// `self ∩ other` — the values in both. Distributes over the terms:
    /// `(A ∪ B) ∩ (C ∪ D)` = `(A∩C) ∪ (A∩D) ∪ (B∩C) ∪ (B∩D)`.
    pub fn intersect(self, other: Ty) -> Ty {
        // A μ is met through its unrolling, so the references inside the result are the
        // recursive type itself rather than a dangling placeholder.
        if self.mu || other.mu {
            return self.unroll().intersect(other.unroll());
        }
        if self.alts.is_none() && other.alts.is_none() {
            return self.intersect_term(other);
        }
        let (a, b) = (self.terms_vec(), other.terms_vec());
        let mut out = Vec::with_capacity(a.len() * b.len());
        for x in &a {
            for y in &b {
                out.push(if x.mu || y.mu {
                    x.clone().intersect(y.clone())
                } else {
                    x.clone().intersect_term(y.clone())
                });
            }
        }
        Ty::from_terms(out)
    }

    /// `¬self` — every value this type excludes. De Morgan over the terms:
    /// `¬(A ∪ B)` = `¬A ∩ ¬B`. Exact for flat terms; a *refined* term's complement
    /// widens to its tag (see [`Ty::negate_term`]), which over-approximates and so can
    /// only ever suppress a warning.
    pub fn negate(self) -> Ty {
        if self.mu {
            return self.unroll().negate();
        }
        if self.alts.is_none() {
            return self.negate_term();
        }
        self.terms_vec()
            .into_iter()
            .map(|t| if t.mu { t.negate() } else { t.negate_term() })
            .fold(Ty::ANY, |acc, n| acc.intersect(n))
    }

    /// `self ⊆ other` — semantic subtyping: is every value of `self` a value of
    /// `other`? Each of `self`'s terms must fit *some* term of `other`. Sound and
    /// deliberately not complete: a term covered jointly by two of `other`'s terms but
    /// by neither alone reads as "not a subtype", which defers rather than warns.
    pub fn is_subtype(&self, other: &Ty) -> bool {
        // A recursive type is compared through its unrolling, COINDUCTIVELY: the pair
        // under comparison is assumed while its unrollings are compared, so meeting it
        // again inside is the assumption holding, not an infinite descent. Sound for
        // subtyping (the greatest fixpoint is the right one for an inclusion between
        // regular types); exact on the shapes inference builds.
        if self.mu || other.mu {
            return with_rec_assumption(self, other, true, |a, b| a.is_subtype(b));
        }
        if self.alts.is_none() && other.alts.is_none() && self.neg.is_none() && other.neg.is_none()
        {
            return self.is_subtype_term(other);
        }
        let other_terms = other.terms_vec();
        self.terms_vec()
            .iter()
            .all(|a| term_covered(a, &other_terms))
    }

    /// **Consistent subtyping** — `self ≲ other`, the gradual reading of `⊆`. A component
    /// of `self` that is UNKNOWN (`any`, or `any` less what a guard excluded — see
    /// [`is_known_only_by_exclusion`](Self::is_known_only_by_exclusion)) is the gradual `?`
    /// wherever it sits, not a static top: a record field the checker could not type, the
    /// elements of a bare `vector`, the keys and values of a bare `map`. Siek & Taha's
    /// definition — `A ≲ B ⟺ ∃ A' ⊑ A. A' ⊆ B`, some way of filling in the unknowns fits —
    /// and in a covariant position the filling that always fits is `never`, so the relation
    /// is `⊆` after that substitution. Arrow parameters are contravariant and left alone: an
    /// unknown there is already the widest domain, which is what a parameter wants.
    ///
    /// This is the inclusion `nest check --strict` reads (`GradualTy::consistent_with_mode`).
    /// Strict exists to flag a value POSITIVELY known to be wider than expected — `number`
    /// into `int` — and a nested unknown is not that: `(assoc b :mark (:end r))` on an
    /// untyped `r` yields a buffer whose `mark` is unknown, and reading that field by plain
    /// inclusion (`any ⊆ nil | int`, false) flagged the value where the SAME unknown handed
    /// over bare would have passed. A positively-known nested type is still read by
    /// inclusion: `vector<number>` into `vector<int>` warns exactly as before. `⊆` itself is
    /// untouched — it is the lattice, and the impossible-guard and exhaustiveness lints need
    /// its exact answer.
    pub fn is_consistent_subtype(&self, other: &Ty) -> bool {
        self.unknowns_as_never().is_subtype(other)
    }

    /// `self` with every nested unknown component replaced by `never` — the most specific
    /// filling of a gradual `?` in a covariant position. Recursive through the data
    /// refinements (elements, tuple slots, map keys and values, record fields). An absent
    /// refinement is the unknown too (`vector` IS `vector<any>`), except where a sibling
    /// slot already describes the same members (a tuple term's elements, a record's
    /// keys and values) — filling in `never` beside it would answer for members it does
    /// not own. A map's SHAPE is a component like any other: a bare `map` (no shape, no
    /// key/value types — what `(assoc x :k v)` on an unknown `x` answers) has unknown
    /// fields, and an OPEN record's undeclared keys are unknown, so both fill to `never`
    /// there — an unknown shape proves nothing about which keys are absent, exactly as
    /// unknown elements prove nothing about what they are. A `map<K, V>` with positively
    /// known `K`/`V` is not touched: it says its keys need not be present. The top level
    /// is not filled either — that is `GradualTy`'s `dynamic` bit, decided by the caller.
    fn unknowns_as_never(&self) -> Ty {
        fn fill(ty: &Ty) -> Ty {
            if ty.is_known_only_by_exclusion() {
                Ty::NEVER
            } else {
                ty.unknowns_as_never()
            }
        }
        let filled = self.terms_vec().into_iter().map(|term| {
            let mut out = term.clone();
            if term.tags & SEQ_BITS != 0 && term.tuple.is_none() && term.list_shape.is_none() {
                out.elem = Some(Arc::new(term.elem.as_deref().map_or(Ty::NEVER, fill)));
            }
            if let Some(elems) = &term.tuple {
                out.tuple = Some(Arc::new(elems.iter().map(fill).collect()));
            }
            if let Some(elems) = &term.list_shape {
                out.list_shape = Some(Arc::new(elems.iter().map(fill).collect()));
            }
            if term.tags & MAP_BIT != 0 {
                match (&term.map_kv, &term.fields) {
                    (Some(kv), _) => out.map_kv = Some(Arc::new((fill(&kv.0), fill(&kv.1)))),
                    (None, Some(shape)) => {
                        let fields = shape
                            .fields
                            .iter()
                            .map(|(name, (ty, required))| (*name, (fill(ty), *required)))
                            .collect();
                        out.fields = Some(Arc::new(RecordShape {
                            fields,
                            rest: fill(&shape.rest),
                        }));
                    }
                    // No shape at all: every key unknown. `rest: never` is the filling —
                    // a shape whose every reading is `never` sits below any record shape.
                    (None, None) => {
                        out.fields = Some(Arc::new(RecordShape {
                            fields: BTreeMap::new(),
                            rest: Ty::NEVER,
                        }));
                    }
                }
            }
            out
        });
        Ty::from_terms(filled.collect())
    }

    /// This term restricted to a single tag — the piece of `self` whose runtime tag is
    /// exactly `tag_bit`, carrying only the refinements that constrain that tag. A term
    /// is the disjoint union of these projections, which is what makes
    /// [`term_is_subtype_of_union`] sound.
    ///
    /// The struct literal is deliberate: a new refinement slot fails to compile here
    /// rather than being silently dropped from the projection (which would be unsound
    /// in the accepting direction), the same protection the manual `PartialEq`/`Hash`
    /// impls get from destructuring.
    fn project_tag(&self, tag_bit: u32) -> Ty {
        fn keep<T>(applies: bool, slot: &Option<Arc<T>>) -> Option<Arc<T>> {
            if applies {
                slot.clone()
            } else {
                None
            }
        }
        Ty {
            tags: tag_bit,
            // A projection is of the POSITIVE part: the subtraction is a whole-term fact,
            // applied by the callers that reason about `P ∖ N`, not per tag.
            neg: None,
            // …and of a self-reference's is every value of the tag — an over-approximation.
            rec_ref: false,
            mu: false,
            arrow: keep(tag_bit & FN_BITS != 0, &self.arrow),
            overload: keep(tag_bit & FN_BITS != 0, &self.overload),
            elem: keep(tag_bit & SEQ_BITS != 0, &self.elem),
            map_kv: keep(tag_bit & MAP_BIT != 0, &self.map_kv),
            fields: keep(tag_bit & MAP_BIT != 0, &self.fields),
            tuple: keep(tag_bit & VECTOR_BIT != 0, &self.tuple),
            list_shape: keep(tag_bit & PAIR_BIT != 0, &self.list_shape),
            lit: keep(tag_bit & KEYWORD_BIT != 0, &self.lit),
            lit_int: keep(tag_bit & INT_BIT != 0, &self.lit_int),
            lit_bool: keep(tag_bit & BOOL_BIT != 0, &self.lit_bool),
            lit_str: keep(tag_bit & STR_BIT != 0, &self.lit_str),
            int_range: if tag_bit & INT_BIT != 0 {
                self.int_range
            } else {
                None
            },
            len: if tag_bit & COUNT_BITS != 0 {
                self.len
            } else {
                None
            },
            alts: None,
        }
    }

    /// Do `self` and `other` share no values? Every pair of terms must be disjoint —
    /// one overlapping pair is one shared value.
    pub fn is_disjoint(&self, other: &Ty) -> bool {
        // Through the unrolling, with the pair assumed NOT disjoint on re-entry: a
        // disjointness that rests on itself is not proven.
        if self.mu || other.mu {
            return with_rec_assumption(self, other, false, |a, b| a.is_disjoint(b));
        }
        if self.alts.is_none() && other.alts.is_none() {
            return self.is_disjoint_term(other);
        }
        let other_terms = other.terms_vec();
        self.terms_vec().iter().all(|a| {
            other_terms.iter().all(|b| {
                if a.mu || b.mu {
                    a.is_disjoint(b)
                } else {
                    a.is_disjoint_term(b)
                }
            })
        })
    }

    /// `self \ other` — values in `self` but not `other`.
    pub fn difference(self, other: Ty) -> Ty {
        self.intersect(other.negate())
    }

    /// `self ⊆ other` — semantic subtyping: is every value of `self` a value of
    /// `other`? Tag-level inclusion first; then, where `other` refines a part
    /// `self` contributes to, `self`'s refinement must satisfy `other`'s:
    /// **functions** via [`Sig::is_subtype`] (contravariant params, covariant
    /// result), **sequences** covariantly on the element type (sound because
    /// Brood sequences are immutable). An unrefined `self` ("any") is *not* a
    /// subtype of a specifically-refined `other`.
    fn is_subtype_term(&self, other: &Ty) -> bool {
        // A self-reference met here is DANGLING (its binder was not unrolled) and so an
        // unknown set: inside another self-reference or `any`, and nothing is inside it.
        if self.rec_ref || other.rec_ref {
            return (self.rec_ref && other.rec_ref) || (self.rec_ref && other.is_top_term());
        }
        if self.tags & other.tags != self.tags {
            return false;
        }
        if self.tags & FN_BITS != 0 {
            // Generalizes the single-arrow case: `other`'s candidate list is
            // `[the one arrow]` when it's unrefined-to-an-overload, so this
            // reproduces the old exact-Sig check unchanged. For a genuine
            // overload, `self` must satisfy *every* signature `other`
            // requires — for each, at least one of `self`'s candidates must
            // be a `Sig::is_subtype` of it (self may carry extra arms beyond
            // what's required — sound, not complete, same conservative shape
            // as `record_fields_is_subtype`). See
            // `docs/type-arrow-intersection.md`.
            let other_candidates = candidate_sigs(other);
            if !other_candidates.is_empty() {
                let self_candidates = candidate_sigs(self);
                for req in &other_candidates {
                    if self_candidates.iter().any(|s| s.is_subtype(req)) {
                        continue;
                    }
                    // No single arm satisfies the requirement — which is not the end of
                    // the question for an INTERSECTION of arrows (ADR-292).
                    if arrows_cover(&self_candidates, req) {
                        continue;
                    }
                    return false;
                }
            }
        }
        if self.tags & SEQ_BITS != 0 {
            if let Some(b) = &other.elem {
                // A tuple has no plain `elem`, but its per-position types taken
                // together are exactly as good a bound: a `tuple<int,int>` IS a
                // `vector<int>` (every element is an int), so derive an
                // equivalent uniform element type from `tuple` when `elem`
                // itself is absent, rather than rejecting outright.
                // …and the same for a list shape on the pair member. Each sequence member
                // `self` admits must be covered — a `(tuple int) | pair` has a bound for its
                // vector and none for its pair, so it has none at all.
                let self_elem = self.elem.clone().or_else(|| {
                    let mut bound = Ty::NEVER;
                    for (bit, shape) in [(VECTOR_BIT, &self.tuple), (PAIR_BIT, &self.list_shape)] {
                        if self.tags & bit == 0 {
                            continue;
                        }
                        let elems = shape.as_ref()?;
                        bound = elems.iter().cloned().fold(bound, |acc, t| acc.union(t));
                    }
                    if self.tags & SEQ_BITS & !(VECTOR_BIT | PAIR_BIT) != 0 {
                        return None; // a set member with no element bound
                    }
                    Some(Arc::new(bound))
                });
                match &self_elem {
                    Some(a) => {
                        if !a.is_subtype(b) {
                            return false;
                        }
                    }
                    None => return false, // self = "any elements" ⊄ a specific elem
                }
            }
        }
        if self.tags & VECTOR_BIT != 0 {
            if let Some(b) = &other.tuple {
                match &self.tuple {
                    Some(a) => {
                        if !tuple_is_subtype(a, b) {
                            return false;
                        }
                    }
                    // self has no specific positional shape (a plain vector, or
                    // only a uniform `elem`) — can't prove it matches an exact
                    // per-position shape `other` requires.
                    None => return false,
                }
            }
        }
        if self.tags & PAIR_BIT != 0 {
            if let Some(b) = &other.list_shape {
                match &self.list_shape {
                    Some(a) => {
                        if !tuple_is_subtype(a, b) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
        }
        if self.tags & MAP_BIT != 0 {
            if let Some(b) = &other.map_kv {
                match &self.map_kv {
                    Some(a) => {
                        // Covariant in both K and V — maps are immutable in Brood.
                        if !a.0.is_subtype(&b.0) || !a.1.is_subtype(&b.1) {
                            return false;
                        }
                    }
                    None => match &self.fields {
                        // A closed record IS a map with keyword keys: it's a subtype of
                        // `map<K,V>` when each of its KEYS fits K and every field value type
                        // fits V. The keys of a closed record are exactly its field names, so
                        // the empty closed record `{}` is inside every `map<K, V>` — which is
                        // what lets a `fold` from `{}` over `(assoc m k v)` settle on the
                        // `map<K, V>` its step builds instead of the union's bare `map`. An
                        // OPEN record's undeclared keys are keywords (a shape comes from a
                        // keyword-keyed literal), so it keeps the "any keyword" reading.
                        Some(shape) => {
                            if shape.is_open() {
                                if !Ty::of(Tag::Keyword).is_subtype(&b.0) {
                                    return false;
                                }
                            } else if !shape
                                .fields
                                .keys()
                                .all(|name| Ty::keyword_lit(*name).is_subtype(&b.0))
                            {
                                return false;
                            }
                            for (vty, _opt) in shape.fields.values() {
                                if !vty.is_subtype(&b.1) {
                                    return false;
                                }
                            }
                            // An OPEN record may carry keys nothing declares, so it is a
                            // `map<K,V>` only if whatever those hold fits `V` too. A
                            // closed record's undeclared keys are absent (`nil`), which
                            // is vacuous — `nil` is not a value the map contains.
                            if shape.is_open() && !shape.rest.is_subtype(&b.1) {
                                return false;
                            }
                        }
                        None => return false, // self = "any map" ⊄ a specific map<K,V>
                    },
                }
            }
            if let Some(b) = &other.fields {
                match &self.fields {
                    Some(a) => {
                        if !record_is_subtype(a, b) {
                            return false;
                        }
                    }
                    None => return false, // self doesn't provably have `other`'s shape
                }
            }
        }
        // Each literal member: every value `self` admits for the tag must be one
        // `other` admits (an unrefined `other` admits all; an open `self` is not a
        // subset of a specific literal set). One rule per independent tag/field.
        if !lit_is_subtype(self.tags & KEYWORD_BIT != 0, &self.lit, &other.lit)
            || !lit_is_subtype(self.tags & INT_BIT != 0, &self.lit_int, &other.lit_int)
            || !lit_is_subtype(self.tags & BOOL_BIT != 0, &self.lit_bool, &other.lit_bool)
            || !lit_is_subtype(self.tags & STR_BIT != 0, &self.lit_str, &other.lit_str)
        {
            return false;
        }
        // …and the intervals: every int `self` admits is within `other`'s, every length.
        if self.tags & INT_BIT != 0 && !Range::subset(self.int_range_eff(), other.int_range_eff()) {
            return false;
        }
        if self.tags & COUNT_BITS != 0 && !Range::subset(self.len_eff(), other.len_eff()) {
            return false;
        }
        true
    }

    /// Do `self` and `other` share no values? (`self ∩ other = ⊥`.) Tag overlap
    /// decides it, with two *precise* exceptions: when the only shared tag is
    /// `keyword` (or, independently, `int`) and both sides pin disjoint literal
    /// sets, no value of that tag satisfies both. This only ever *adds*
    /// genuinely-disjoint cases (a literal set is an exact enumeration, not an
    /// approximation), so it can't raise a false warning — advisory-soundness
    /// holds.
    fn is_disjoint_term(&self, other: &Ty) -> bool {
        if self.rec_ref || other.rec_ref {
            return false; // an unknown set is never PROVABLY disjoint
        }
        let shared = self.tags & other.tags;
        if shared == 0 {
            return true;
        }
        // When the sole shared tag is a literal kind and both sides pin disjoint
        // sets, no value of that tag satisfies both. One rule per independent tag.
        if let Some(d) = lit_disjoint(shared == KEYWORD_BIT, &self.lit, &other.lit) {
            return d;
        }
        if let Some(d) = lit_disjoint(shared == INT_BIT, &self.lit_int, &other.lit_int) {
            return d;
        }
        if let Some(d) = lit_disjoint(shared == BOOL_BIT, &self.lit_bool, &other.lit_bool) {
            return d;
        }
        if let Some(d) = lit_disjoint(shared == STR_BIT, &self.lit_str, &other.lit_str) {
            return d;
        }
        // The sole shared tag is `int` and the intervals do not meet; or every shared tag
        // is countable and the lengths do not meet.
        if shared == INT_BIT && Range::disjoint(self.int_range_eff(), other.int_range_eff()) {
            return true;
        }
        if shared & !COUNT_BITS == 0 && Range::disjoint(self.len_eff(), other.len_eff()) {
            return true;
        }
        // Two tuple shapes are provably disjoint if their arities differ (a
        // vector value has exactly one length, so it can't be both a 2-tuple
        // and a 3-tuple) or if any single position's types are disjoint (a
        // value satisfying both shapes would need every position to satisfy
        // both at once). Same soundness basis as the literal-set cases above
        // — this only ever *adds* a genuinely-disjoint verdict, never a false
        // one.
        if shared == VECTOR_BIT {
            if let (Some(a), Some(b)) = (&self.tuple, &other.tuple) {
                if a.len() != b.len() {
                    return true;
                }
                // Only a PROVEN disjointness returns here. Returning the negative answer
                // too would skip the subtraction check below, which is the one that knows
                // `(tuple int|string) ∖ (tuple int)` shares nothing with `(tuple int)`.
                if a.iter().zip(b.iter()).any(|(x, y)| x.is_disjoint(y)) {
                    return true;
                }
            }
        }
        // …and two list shapes, the same way (a list has exactly one length).
        if shared == PAIR_BIT {
            if let (Some(a), Some(b)) = (&self.list_shape, &other.list_shape) {
                if a.len() != b.len() || a.iter().zip(b.iter()).any(|(x, y)| x.is_disjoint(y)) {
                    return true;
                }
            }
        }
        // Two NON-EMPTY lists are provably disjoint when their element types are: a
        // `list<T>` is the `pair` tag alone — the empty list is `nil`, a separate tag —
        // so every value of `list<int>` has a first element, and no first element is
        // both an `int` and a `string`. The same fact `intersect` states by making
        // `list<int> ∩ list<string>` the uninhabited `list<never>`; the two must agree
        // or the argument check (`!is_disjoint`) and the lattice contradict each other.
        if shared == (1u32 << bit(Tag::Pair)) {
            if let (Some(a), Some(b)) = (self.elem_ty(), other.elem_ty()) {
                if a.is_disjoint(&b) {
                    return true;
                }
            }
        }
        // Two record shapes are provably disjoint if they both constrain some
        // field, that field is **required** on at least one side (so any value
        // must carry it — an optional-on-both field could just be absent), and
        // the two field types are disjoint (no single value can be both). Same
        // soundness basis as the tuple case: only ever *adds* a genuine disjoint
        // verdict. Open records mean a field only one side mentions never yields
        // disjointness (the other side permits the extra field freely). This is
        // what lets a guard-refined base — `(record :age int)` from
        // `(if (int? (get r :age)) …)` — flag a call wanting `(record :age string)`.
        if shared == MAP_BIT {
            if let (Some(a), Some(b)) = (&self.fields, &other.fields) {
                if records_are_disjoint(a, b) {
                    return true;
                }
            }
        }
        // Subtractions (ADR-288): `(Pa ∖ Na) ∩ (Pb ∖ Nb)` is empty exactly when what the
        // positives share is wholly subtracted — `vector<int> ∩ ¬vector<int>` is the
        // simplest case, and without this `is_disjoint` and `intersect` disagreed about it.
        if self.neg.is_some() || other.neg.is_some() {
            let mut negs: Vec<Ty> = Vec::new();
            for n in [&self.neg, &other.neg].into_iter().flatten() {
                negs.extend(n.iter().cloned());
            }
            let shared_positive = self.positive().intersect(other.positive());
            if shared_positive
                .terms_vec()
                .iter()
                .all(|t| term_is_subtype_of_union(t, &negs))
            {
                return true;
            }
        }
        false
    }

    /// The terms of a *multi*-term type, or `None` for a single term — the display
    /// hook, kept beside [`Ty::single`] so the two read as one decision.
    pub(crate) fn alt_terms(&self) -> Option<Vec<Ty>> {
        self.alts.is_some().then(|| self.terms_vec())
    }

    /// A recursive type's body — the same terms, without the binder — for the
    /// renderer, which prints the binder itself.
    /// The element refinement this one term carries, if any — for the renderer's mixed
    /// join, which lists tags and must not drop what a sequence member says of its
    /// elements.
    pub(crate) fn term_elem_for_display(&self) -> Option<Ty> {
        self.elem.as_deref().cloned()
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

    /// The length suffix when `tag` is countable.
    pub(crate) fn len_suffix_for(&self, tag: Tag) -> String {
        if (1u32 << bit(tag)) & COUNT_BITS != 0 {
            self.len_suffix()
        } else {
            String::new()
        }
    }

    /// A single term's int interval, for `to_source`.
    pub(crate) fn int_interval_for_source(&self) -> Option<Range> {
        self.single()?.int_range
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

    // ---- recursive types (ADR-349) ------------------------------------------------

    /// The self-reference term: `X` inside `μX. …`. Meaningful only inside a type whose
    /// [`mu`](Ty::mu) is set; on its own it is an unknown set.
    pub fn rec_ref() -> Ty {
        Ty {
            rec_ref: true,
            ..Ty::flat(UNIVERSE)
        }
    }

    /// Is this the self-reference term?
    pub fn is_rec_ref(&self) -> bool {
        self.rec_ref
    }

    /// Is this a recursive type — a μ binder?
    pub fn is_recursive(&self) -> bool {
        self.mu
    }

    /// `μX. body` — `body` with its self-references bound to the whole. A body with no
    /// self-reference is not recursive and comes back as it is.
    pub fn mu(body: Ty) -> Ty {
        Ty { mu: true, ..body }.normalise_mu()
    }

    /// The canonical length slot beside a POSITIONAL shape: a tuple or a list shape is
    /// its arity, so a stored length beside it is redundant and goes.
    fn normalise_len(mut self) -> Ty {
        let shaped = (self.tags & COUNT_BITS == VECTOR_BIT && self.tuple.is_some())
            || (self.tags & COUNT_BITS == PAIR_BIT && self.list_shape.is_some());
        if shaped {
            self.len = None;
        }
        self
    }

    /// Drop the binder when nothing refers to it any more (a widening or a merge took
    /// the references with it), so a non-recursive type never claims to be one.
    fn normalise_mu(mut self) -> Ty {
        if self.mu && !self.has_rec_ref() {
            self.mu = false;
        }
        self
    }

    /// Is `any` — every tag, no refinement, no subtraction — as a single term?
    fn is_top_term(&self) -> bool {
        self.tags == UNIVERSE
            && !self.rec_ref
            && self.arrow.is_none()
            && self.overload.is_none()
            && self.elem.is_none()
            && self.map_kv.is_none()
            && self.fields.is_none()
            && self.tuple.is_none()
            && self.list_shape.is_none()
            && self.lit.is_none()
            && self.lit_int.is_none()
            && self.lit_bool.is_none()
            && self.lit_str.is_none()
            && self.neg.is_none()
            && self.int_range.is_none()
            && self.len.is_none()
    }

    /// Does a self-reference occur in this type's refinement tree, outside any nested
    /// binder?
    pub fn has_rec_ref(&self) -> bool {
        // A term that is itself a binder (a recursive type kept as one alternative of a
        // union) is closed: its references are its own.
        self.body_terms().iter().any(|term| {
            !term.mu && (term.rec_ref || term.each_nested(&mut |t| !t.mu && t.has_rec_ref()))
        })
    }

    /// Does `pred` hold of some Ty directly nested in this term's refinements — an
    /// element, a key or value, a field, a position, an arrow's parameter or result, a
    /// subtracted term? (One level; the caller recurses.)
    fn each_nested(&self, pred: &mut dyn FnMut(&Ty) -> bool) -> bool {
        if let Some(e) = &self.elem {
            if pred(e) {
                return true;
            }
        }
        if let Some(kv) = &self.map_kv {
            if pred(&kv.0) || pred(&kv.1) {
                return true;
            }
        }
        if let Some(f) = &self.fields {
            if f.fields.values().any(|(t, _)| pred(t)) || pred(&f.rest) {
                return true;
            }
        }
        for shape in [&self.tuple, &self.list_shape].into_iter().flatten() {
            if shape.iter().any(&mut *pred) {
                return true;
            }
        }
        let sigs: Vec<&Sig> = self
            .arrow
            .iter()
            .map(|s| s.as_ref())
            .chain(self.overload.iter().flat_map(|o| o.iter()))
            .collect();
        for sig in sigs {
            if sig.params.iter().any(&mut *pred)
                || sig.rest.as_ref().is_some_and(&mut *pred)
                || pred(&sig.ret)
            {
                return true;
            }
        }
        if let Some(negs) = &self.neg {
            if negs.iter().any(&mut *pred) {
                return true;
            }
        }
        false
    }

    /// This type with every directly nested Ty of every term replaced by `f` of it (a
    /// nested binder is closed and passed through untouched); the terms are re-unioned,
    /// since `f` may turn a term into a union.
    fn map_nested(&self, f: &mut dyn FnMut(&Ty) -> Ty) -> Ty {
        // A binder is closed: nothing inside it is this level's to map.
        if self.mu {
            return self.clone();
        }
        let terms: Vec<Ty> = self
            .body_terms()
            .into_iter()
            .map(|term| {
                if term.mu {
                    return term;
                }
                let mut out = term.clone();
                out.elem = term.elem.as_ref().map(|e| Arc::new(f(e)));
                out.map_kv = term
                    .map_kv
                    .as_ref()
                    .map(|kv| Arc::new((f(&kv.0), f(&kv.1))));
                out.fields = term.fields.as_ref().map(|shape| {
                    Arc::new(RecordShape {
                        fields: shape
                            .fields
                            .iter()
                            .map(|(k, (t, req))| (*k, (f(t), *req)))
                            .collect(),
                        rest: f(&shape.rest),
                    })
                });
                out.tuple = term
                    .tuple
                    .as_ref()
                    .map(|ts| Arc::new(ts.iter().map(&mut *f).collect()));
                out.list_shape = term
                    .list_shape
                    .as_ref()
                    .map(|ts| Arc::new(ts.iter().map(&mut *f).collect()));
                let map_sig = |s: &Sig, f: &mut dyn FnMut(&Ty) -> Ty| Sig {
                    params: s.params.iter().map(&mut *f).collect(),
                    rest: s.rest.as_ref().map(&mut *f),
                    ret: f(&s.ret),
                    ..s.clone()
                };
                out.arrow = term.arrow.as_ref().map(|s| Arc::new(map_sig(s, f)));
                out.overload = term
                    .overload
                    .as_ref()
                    .map(|o| Arc::new(o.iter().map(|s| map_sig(s, f)).collect()));
                out.neg = term
                    .neg
                    .as_ref()
                    .map(|ns| Arc::new(ns.iter().map(&mut *f).collect()));
                out
            })
            .collect();
        let mut out = terms.into_iter().fold(Ty::NEVER, Ty::union);
        if self.mu {
            out.mu = true;
        }
        out
    }

    /// One level of the recursion: this type with each self-reference replaced by the
    /// type itself. Not a binder afterwards — the copies inside are. A non-recursive
    /// type is its own unrolling.
    pub fn unroll(&self) -> Ty {
        if !self.mu {
            return self.clone();
        }
        let whole = self.clone();
        let body = Ty {
            mu: false,
            ..self.clone()
        };
        body.subst_rec(&whole)
    }

    /// Every self-reference in this type (outside a nested binder) replaced by `with`.
    fn subst_rec(&self, with: &Ty) -> Ty {
        if self.mu {
            return self.clone(); // a nested binder's references are its own
        }
        // A self-reference TERM is replaced by the whole of `with` (a union), so the
        // terms are rebuilt through `union`.
        let mut terms: Vec<Ty> = Vec::new();
        for term in self.body_terms() {
            if term.rec_ref {
                terms.push(with.clone());
            } else if term.mu {
                terms.push(term);
            } else {
                terms.push(term.map_nested(&mut |t| t.subst_rec(with)));
            }
        }
        let mut out = terms.into_iter().fold(Ty::NEVER, Ty::union);
        if self.mu {
            out.mu = true;
        }
        out
    }

    /// **Fold an ascent into a recursive type** (ADR-349). When one round of a fixpoint
    /// computes `next` from `prev` and `prev` appears INSIDE `next` — as an element, a
    /// value, a field — the round is `next = G[prev]` for the context `G`, and
    /// `μX. G[X]` is the candidate the iteration is climbing towards: `Some` of it,
    /// with every nested occurrence of `prev` replaced by the self-reference. `None`
    /// when `prev` is flat (a bare tag set nests nothing worth folding), empty, or
    /// absent from `next`.
    ///
    /// A candidate is not an answer: the caller runs the next round on it, and only
    /// when that round folds back to the candidate — `G[μ]` is `μ` — is it accepted,
    /// which makes it a post-fixpoint of the round and therefore above the least one
    /// (sound). Until then the ascent continues from the round's own value, which is
    /// above the un-folded sequence since `prev ⊆ μ` and `G` is monotone.
    pub fn fold_recursive(prev: &Ty, next: &Ty) -> Option<Ty> {
        if prev.is_never() || prev == next || prev.terms_vec().iter().all(Ty::is_flat_term) {
            return None;
        }
        let mut hits = 0usize;
        let folded = next.replace_nested(prev, &mut hits);
        if hits == 0 {
            return None;
        }
        Some(Ty::mu(folded))
    }

    /// A term with no structural refinement, subtraction or self-reference.
    fn is_flat_term(&self) -> bool {
        !self.rec_ref
            && self.arrow.is_none()
            && self.overload.is_none()
            && self.elem.is_none()
            && self.map_kv.is_none()
            && self.fields.is_none()
            && self.tuple.is_none()
            && self.list_shape.is_none()
            && self.neg.is_none()
    }

    /// This type with every nested occurrence of `target` (at any depth, outside a
    /// nested binder; never the whole) replaced by the self-reference term.
    fn replace_nested(&self, target: &Ty, hits: &mut usize) -> Ty {
        self.map_nested(&mut |t| {
            if t == target {
                *hits += 1;
                Ty::rec_ref()
            } else {
                t.replace_nested(target, hits)
            }
        })
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

    /// The positive part, for rendering — see [`Ty::positive`].
    pub(super) fn positive_for_display(&self) -> Ty {
        self.positive()
    }

    /// Whether this single term is empty, for rendering — see [`Ty::term_is_never`].
    pub(super) fn term_is_empty_for_display(&self) -> bool {
        self.term_is_never()
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

/// Record-shape subtyping: is `self`'s field map a subtype of `other`'s?
/// **Sound but deliberately not complete** (`docs/types.md` contract #5,
/// `docs/type-records.md`): for every field `other` declares, `self` must
/// also declare it (required if `other` requires it) with a covariant field
/// type. A field `other` doesn't declare imposes no constraint (open records
/// — `self` may carry extra fields freely; this is the width-subtyping
/// direction). **Conservative on purpose:** if `self` simply doesn't declare
/// a field `other` does (even one `other` marks optional), this returns
/// `false` rather than trying to prove the relationship holds anyway — a
/// missed subtype relation is fine (incomplete), but never claiming one that
/// doesn't hold (unsound) is not negotiable.
fn record_is_subtype(a: &RecordShape, b: &RecordShape) -> bool {
    // Quantify over every key EITHER declares, comparing what a read yields, then the
    // undeclared remainder. One rule covers what used to need three: an optional field
    // reads as `T | nil` (so a shape omitting it is still a subtype of one that marks it
    // optional), a required field `b` declares and `a` does not fails because `a`'s
    // reading is `a.rest` (`nil` closed, `any` open) which is not the required type, and
    // width subtyping falls out of `a.rest ⊆ b.rest` — `nil ⊆ any` (closed is a subtype
    // of open) but not the reverse.
    for key in a.keys_with(b) {
        if !a.field_ty(key).is_subtype(&b.field_ty(key)) {
            return false;
        }
    }
    a.rest.is_subtype(&b.rest)
}

/// Do two record shapes share no value? When some key's readings are disjoint, no map
/// can satisfy both — which is exactly how a **closed** shape discriminates a tagged
/// union: `{ok: int}` says `:error` is absent (`nil`), `{error: string}` says it is a
/// string, and `nil ∩ string = ⊥`.
///
/// Sound in the usual direction: this only ever *adds* a genuine disjointness verdict
/// (a field's reading is an over-approximation, so disjoint readings mean disjoint
/// values), and two open shapes disagreeing on nothing they both declare stay
/// overlapping, as before.
fn records_are_disjoint(a: &RecordShape, b: &RecordShape) -> bool {
    a.keys_with(b)
        .into_iter()
        .any(|key| a.field_ty(key).is_disjoint(&b.field_ty(key)))
}

/// The **exact** intersection of two record shapes: per key, what both readings admit;
/// for the remainder, what both rests admit. `None` when no value can satisfy both (some
/// key's readings are disjoint), which the caller turns into "not a map at all".
///
/// Exact rather than widened because this is the narrowing a guard performs — `(if
/// (int? (get r :age)) …)` intersects the incoming shape with `(record &open :age int)`,
/// and dropping the refinement there would lose the very fact the guard established.
fn intersect_records(a: &RecordShape, b: &RecordShape) -> Option<RecordShape> {
    let mut fields = BTreeMap::new();
    for key in a.keys_with(b) {
        let ty = a.field_ty(key).intersect(b.field_ty(key));
        if ty.is_never() {
            return None;
        }
        // Required on either side means required in the intersection: a value must carry
        // it. `field_ty` already folded `nil` into an optional reading, so a field that
        // stays optional keeps that `nil` and reads the same.
        let required = a.fields.get(&key).is_some_and(|(_, r)| *r)
            || b.fields.get(&key).is_some_and(|(_, r)| *r);
        fields.insert(key, (ty, required));
    }
    Some(RecordShape {
        fields,
        rest: a.rest.clone().intersect(b.rest.clone()),
    })
}

/// The exact intersection of two positional shapes: same arity, then per position.
/// `None` when no vector can be both — a differing arity, or a position whose types
/// share nothing (unlike a uniform element type, where `never` still admits the empty
/// vector, a tuple position must actually hold a value).
fn intersect_tuples(a: &[Ty], b: &[Ty]) -> Option<Vec<Ty>> {
    if a.len() != b.len() {
        return None;
    }
    let mut out = Vec::with_capacity(a.len());
    for (x, y) in a.iter().zip(b) {
        let t = x.clone().intersect(y.clone());
        if t.is_never() {
            return None;
        }
        out.push(t);
    }
    Some(out)
}

/// `self <: other` for two tuple shapes: exact arity match (unlike a record's
/// open width-subtyping, a tuple's arity *is* its shape — a 2-tuple isn't a
/// subtype of a 3-tuple, and vice versa), then covariant per position — sound
/// because Brood vectors are immutable, same reasoning as element-covariant
/// sequences.
/// The subtractions of two terms, combined — for the intersection rule
/// `(Pa ∖ Na) ∩ (Pb ∖ Nb) = (Pa ∩ Pb) ∖ (Na ∪ Nb)`. Deduplicated, since the same term is
/// commonly subtracted twice (`¬T ∩ ¬T`), and capped so a chain of intersections cannot
/// grow an unbounded list — past the cap the extra subtractions are DROPPED, which widens
/// the type and is therefore the safe direction to lose precision in.
fn union_negs(a: &Option<Arc<Vec<Ty>>>, b: &Option<Arc<Vec<Ty>>>) -> Option<Arc<Vec<Ty>>> {
    match (a, b) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (Some(x), Some(y)) => {
            let mut out: Vec<Ty> = x.as_ref().clone();
            for t in y.iter() {
                if out.len() >= MAX_NEG_TERMS {
                    break;
                }
                if !out.contains(t) {
                    out.push(t.clone());
                }
            }
            // A SET, not a sequence: `Ty` derives its equality and hash from its slots, so
            // `¬A ∩ ¬B` and `¬B ∩ ¬A` would otherwise be unequal types denoting one set —
            // the same defect ADR-270 fixed for literal spellings. Sorted by rendering,
            // which is deterministic and needs no `Ord` on `Ty`.
            out.sort_by_key(Ty::to_string);
            Some(Arc::new(out))
        }
    }
}

/// Does an **intersection of arrows** satisfy a required arrow together, when no single one
/// of them does (ADR-292)?
///
/// A function that maps `int → int` *and* `bool → bool` does map `int|bool → int|bool`, but
/// neither conjunct says so alone: `(int → int)` is not below `(int|bool → int|bool)`,
/// because an arrow's domain is **contravariant** and `int|bool ⊄ int`. Only the two
/// together cover it. That is the whole reason the arrow rule exists.
///
/// ```text
/// ⋀_{i∈P} (Sᵢ → Tᵢ)  ≤  (S → T)   ⟺   ∀ S' ⊆ P:
///     S ⊆ ⋃_{i∈P∖S'} Sᵢ      or      ⋂_{i∈S'} Tᵢ ⊆ T
/// ```
///
/// — read as: for any way the arms divide, either the ones you set aside still cover the
/// required domain, or the ones you kept already agree on a result inside the required one.
/// The empty intersection is the top type, so an `S'` of nothing demands the domain be
/// covered by every arm together.
///
/// The domains are **products** (a signature takes several parameters), so this reuses
/// [`tuple_covered_by`] rather than inventing a second covering rule — the same question
/// asked of the same shape, which is what keeps the two answers from drifting.
///
/// Conservative where the shape is not a plain fixed arity: an `&optional` or `&` rest on
/// either side declines, which can only refuse a true containment, never accept a false one.
fn arrows_cover(candidates: &[Sig], req: &Sig) -> bool {
    if !req.optional.is_empty() || req.rest.is_some() {
        return false;
    }
    let arms: Vec<&Sig> = candidates
        .iter()
        .filter(|c| c.params.len() == req.params.len() && c.optional.is_empty() && c.rest.is_none())
        .collect();
    // A different arity accepts a different call, so it cannot help cover this one.
    if arms.is_empty() || arms.len() > 6 {
        return false;
    }
    for mask in 0..(1u32 << arms.len()) {
        // The arms NOT kept must still cover the required domain…
        let set_aside: Vec<Vec<Ty>> = arms
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) == 0)
            .map(|(_, c)| c.params.clone())
            .collect();
        if tuple_covered_by(&req.params, &set_aside) {
            continue;
        }
        // …or the arms kept must agree on a result inside the required one. The empty
        // intersection is `any`, which is inside the requirement only if it asks for `any`.
        let kept = arms
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .fold(Ty::ANY, |acc, (_, c)| acc.intersect(c.ret.clone()));
        if kept.is_subtype(&req.ret) {
            continue;
        }
        return false;
    }
    true
}

/// Is a **product** contained in a union of products? — the set-theoretic rule for tuples.
///
/// Componentwise containment is *not* the answer, and assuming it is the classic error:
/// `(int|string, int|string)` covers each component against `{(int,int), (string,string)}`
/// and yet `(int, string)` belongs to neither. The rule that is correct splits the
/// candidates every possible way:
///
/// ```text
/// (A₁ … Aₙ) ⊆ ⋃_{j∈J} (B₁ⱼ … Bₙⱼ)   ⟺   ∀ J' ⊆ J:
///     A₁ ⊆ ⋃_{j∈J'} B₁ⱼ    or    (A₂ … Aₙ) ⊆ ⋃_{j∈J∖J'} (B₂ⱼ … Bₙⱼ)
/// ```
///
/// — read as: for any way the candidates might divide, either they already cover the first
/// component, or the ones they do not cover it with must cover everything after it. The
/// base case is the empty tuple, which is covered exactly when some candidate remains.
///
/// Exponential in the candidate count, which is why it is safe here: a `Ty` holds at most
/// [`MAX_TY_TERMS`] alternatives, so `J` is tiny, and the cap below refuses rather than
/// blows up if that ever stops being true.
fn tuple_covered_by(a: &[Ty], candidates: &[Vec<Ty>]) -> bool {
    if a.is_empty() {
        // `()` is covered iff some candidate is still in play — it is the only value of
        // its type, so any surviving `()` candidate contains it.
        return !candidates.is_empty();
    }
    if candidates.len() > 8 {
        return false; // conservative: refuse a pathological fan-out rather than enumerate it
    }
    let n = candidates.len();
    for mask in 0..(1u32 << n) {
        let mut first = Ty::NEVER;
        let mut rest: Vec<Vec<Ty>> = Vec::new();
        for (j, cand) in candidates.iter().enumerate() {
            if mask & (1 << j) != 0 {
                first = first.union(cand[0].clone());
            } else {
                rest.push(cand[1..].to_vec());
            }
        }
        if a[0].is_subtype(&first) {
            continue;
        }
        if tuple_covered_by(&a[1..], &rest) {
            continue;
        }
        return false;
    }
    true
}

/// Is one term — subtractions and all — contained in the union of `others`?
///
/// `(P ∖ N) ⊆ ⋃B` is `P ⊆ ⋃B ∪ N`: whatever `P` holds that the candidates do not cover is
/// acceptable exactly when this term already subtracts it. That one rearrangement is what
/// lets subtractions ride the covering procedure instead of needing one of their own.
///
/// A candidate that *itself* subtracts something is dropped rather than folded in —
/// `⋃(Pⱼ ∖ Nⱼ)` is not a union of positives, and treating it as one would over-accept.
/// Dropping can only turn a true answer into `false`: incompleteness, never unsoundness.
fn term_covered(a: &Ty, others: &[Ty]) -> bool {
    if a.term_is_never() {
        return true; // the empty set is below everything
    }
    let mut candidates: Vec<Ty> = others.iter().filter(|b| b.neg.is_none()).cloned().collect();
    if let Some(negs) = &a.neg {
        candidates.extend(negs.iter().cloned());
    }
    if term_is_subtype_of_union(&a.positive(), &candidates) {
        return true;
    }
    // A candidate that itself subtracts is not a positive and cannot join the union above,
    // but it can still contain `a` on its own: `a ⊆ (P ∖ N)` exactly when `a ⊆ P` and `a`
    // meets none of `N`. Dropping these outright failed `a ⊆ a ∪ b` as soon as the union
    // absorbed into a subtracting term — `int ⊆ int ∪ ¬vector<int>` came out false.
    others.iter().filter(|b| b.neg.is_some()).any(|b| {
        term_covered(a, std::slice::from_ref(&b.positive()))
            && b.neg
                .as_deref()
                .is_some_and(|negs| negs.iter().all(|n| a.is_disjoint(n)))
    })
}

/// Is one term contained in the *union* of `others`?
///
/// The direct question — does some single `other` contain `a`? — is sound but
/// incomplete, and the incompleteness costs a **false positive**: the checker warns
/// about a call that is in fact fine. `int | vector<int>` is a single term (the union
/// merged exactly), and it sits inside `(int | vector<string>) | vector<int>` only once
/// you notice that its two halves land in *different* alternatives.
///
/// A term is the disjoint union of its per-tag projections, so it suffices to place
/// each projection in some alternative: `a = ⋃ₜ a|ₜ ⊆ ⋃ others`. That is sound, and
/// strictly sharper than the single-alternative test (which it keeps as a fast path,
/// since a term contained in one alternative has every projection contained there too).
///
/// Still incomplete where one *tag's* refinement is split across alternatives —
/// `vector<int|string>` against `vector<int> | vector<string>`. Deciding that needs the
/// emptiness procedure a full negation type would bring; until then this direction only
/// under-reports precision, never over-accepts.
fn term_is_subtype_of_union(a: &Ty, others: &[Ty]) -> bool {
    if a.tags == 0 {
        return true; // `never` is below everything
    }
    // A term that is itself a binder (a recursive type kept as one alternative) is
    // compared through the coinductive `is_subtype`, never slot by slot.
    if others.iter().any(|b| {
        if a.mu || b.mu {
            a.is_subtype(b)
        } else {
            a.is_subtype_term(b)
        }
    }) {
        return true;
    }
    // Iterate the tag table rather than isolating set bits: `x & x.wrapping_neg()` draws a
    // clippy lint whose suggested replacement (`isolate_lowest_one`) is unstable below
    // Rust 1.98, and this crate builds on 1.95 — the two cannot both be satisfied by a bit
    // trick. `ALL_TAGS` is 23 entries, this is not a hot path, and it is the idiom the
    // rest of the module already reads by.
    for tag in ALL_TAGS {
        let tag_bit = 1u32 << bit(tag);
        if a.tags & tag_bit == 0 {
            continue;
        }
        let part = a.project_tag(tag_bit);
        if others.iter().any(|b| part.is_subtype_term(b)) {
            continue;
        }
        // No single alternative covers this tag's projection. For a **tuple** that is not
        // the end of the question: a product can be covered by several alternatives
        // together, which is the one shape where splitting a refinement across a union is
        // sound — `(int|string)` is covered by `(int)` and `(string)` between them, because
        // a 1-tuple holds exactly one value and it lands in one or the other.
        //
        // Only tuples. A `vector<A>` covered by `vector<B₁> | vector<B₂>` needs `A ⊆ Bⱼ`
        // for a single j, which the check above already asked: an arbitrary-length vector
        // can hold one element outside B₁ and another outside B₂, so it escapes both.
        // The fixed arity is exactly what makes the product case different.
        if tag_bit == VECTOR_BIT {
            if let Some(elems) = part.tuple_elems() {
                let candidates: Vec<Vec<Ty>> = others
                    .iter()
                    .filter(|b| b.tags & VECTOR_BIT != 0)
                    .filter_map(|b| b.tuple_elems().cloned())
                    // A different arity shares no value with this one.
                    .filter(|c| c.len() == elems.len())
                    .collect();
                if tuple_covered_by(elems, &candidates) {
                    continue;
                }
            }
        }
        // A list shape is a product too.
        if tag_bit == PAIR_BIT {
            if let Some(elems) = part.list_shape_elems() {
                let candidates: Vec<Vec<Ty>> = others
                    .iter()
                    .filter(|b| b.tags & PAIR_BIT != 0)
                    .filter_map(|b| b.list_shape_elems().cloned())
                    .filter(|c| c.len() == elems.len())
                    .collect();
                if tuple_covered_by(elems, &candidates) {
                    continue;
                }
            }
        }
        return false;
    }
    true
}

fn tuple_is_subtype(self_elems: &[Ty], other_elems: &[Ty]) -> bool {
    self_elems.len() == other_elems.len()
        && self_elems
            .iter()
            .zip(other_elems)
            .all(|(s, o)| s.is_subtype(o))
}

/// The candidate signatures a function-tagged `Ty` carries: `overload`'s list
/// if present (2+ sigs), else `arrow` as a one-element list, else empty ("any
/// function" — no info). The shared extraction [`intersect_arrows`] and
/// [`Ty::is_subtype`] both build on.
fn candidate_sigs(ty: &Ty) -> Vec<Sig> {
    if let Some(sigs) = &ty.overload {
        sigs.as_ref().clone()
    } else if let Some(sig) = &ty.arrow {
        vec![sig.as_ref().clone()]
    } else {
        Vec::new()
    }
}

/// `self`'s and `other`'s arrow refinements, intersected — the composition
/// rule for intersection *types*: a value satisfying both `self` and `other`
/// must satisfy every signature contributed by *either* side (`f : (A→B) ∧
/// (C→D)` means both arrows apply to `f`), so the result is the deduplicated
/// union of the two candidate lists. A side with **no** candidates ("any
/// function") leaves the other's candidates untouched — this reproduces
/// today's exact behaviour for `(and fn (int -> int))` and for two identical
/// arrows. Collapses to `(Some(sig), None)` when exactly one distinct
/// signature survives (the common case — every existing single-arrow
/// consumer is unaffected), else `(None, Some(overload_list))`. See
/// `docs/type-arrow-intersection.md`.
fn intersect_arrows(a: &Ty, b: &Ty) -> (Option<Arc<Sig>>, Option<Arc<Vec<Sig>>>) {
    let sa = candidate_sigs(a);
    let sb = candidate_sigs(b);
    if sa.is_empty() {
        return (b.arrow.clone(), b.overload.clone());
    }
    if sb.is_empty() {
        return (a.arrow.clone(), a.overload.clone());
    }
    let mut combined = sa;
    for sig in sb {
        if !combined.contains(&sig) {
            combined.push(sig);
        }
    }
    if combined.len() == 1 {
        (
            Some(Arc::new(combined.into_iter().next().expect("len == 1"))),
            None,
        )
    } else {
        (None, Some(Arc::new(combined)))
    }
}

thread_local! {
    /// The pairs of recursive types under comparison right now — the coinductive
    /// assumptions of [`with_rec_assumption`].
    static REC_ASSUMPTIONS: std::cell::RefCell<Vec<(Ty, Ty)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Compare `a` and `b` — at least one recursive — through their unrollings, under the
/// assumption that the pair itself satisfies the relation: a re-encounter of the same
/// pair inside answers `assumed` (true for an inclusion, false for a disjointness —
/// the answer that is sound when the question rests on itself). Regular types have
/// finitely many distinct pairs of subterms, so the descent terminates.
fn with_rec_assumption(a: &Ty, b: &Ty, assumed: bool, relate: impl Fn(&Ty, &Ty) -> bool) -> bool {
    let seen = REC_ASSUMPTIONS.with(|s| s.borrow().iter().any(|(x, y)| x == a && y == b));
    if seen {
        return assumed;
    }
    REC_ASSUMPTIONS.with(|s| s.borrow_mut().push((a.clone(), b.clone())));
    let out = relate(&a.unroll(), &b.unroll());
    REC_ASSUMPTIONS.with(|s| {
        s.borrow_mut().pop();
    });
    out
}

/// Would the widening union of these two terms lose nothing — is their union
/// representable as one term?
///
/// It is, unless some refinement slot is *contested*: both sides contribute the tags
/// that slot refines, and they carry different refinements there. That is exactly the
/// case [`merge_union`] answers with `None` (widen to "any of that tag"), and exactly
/// the case a second term now exists to hold instead. One rule per slot, matching the
/// slots and tag masks `union_term` uses.
fn merge_is_exact(a: &Ty, b: &Ty) -> bool {
    // A self-reference merges with nothing but another self-reference: merged into a
    // term it would either drop the term (unsound) or read as `any` (imprecise) — kept
    // as its own alternative it stays exactly what it is.
    if a.rec_ref != b.rec_ref {
        return false;
    }
    fn contested<T: PartialEq>(
        a_present: bool,
        a: &Option<Arc<T>>,
        b_present: bool,
        b: &Option<Arc<T>>,
    ) -> bool {
        a_present && b_present && a != b
    }
    // A subtraction on either side makes a merged union unsound to compute here: the
    // merge combines POSITIVE slots, and `(P₁ ∖ N) ∪ P₂` is not `(P₁ ∪ P₂) ∖ N`. Refusing
    // keeps both terms as alternatives, which is exact (ADR-288).
    if a.neg.is_some() || b.neg.is_some() {
        return false;
    }
    let (fa, fb) = (a.tags & FN_BITS != 0, b.tags & FN_BITS != 0);
    let (sa, sb) = (a.tags & SEQ_BITS != 0, b.tags & SEQ_BITS != 0);
    let (ma, mb) = (a.tags & MAP_BIT != 0, b.tags & MAP_BIT != 0);
    let (va, vb) = (a.tags & VECTOR_BIT != 0, b.tags & VECTOR_BIT != 0);
    let (pa, pb) = (a.tags & PAIR_BIT != 0, b.tags & PAIR_BIT != 0);
    !contested(fa, &a.arrow, fb, &b.arrow)
        && !contested(fa, &a.overload, fb, &b.overload)
        && (!contested(sa, &a.elem, sb, &b.elem) || elem_union_exact(a, b).is_some())
        && !contested(ma, &a.map_kv, mb, &b.map_kv)
        && !contested(ma, &a.fields, mb, &b.fields)
        && (!contested(va, &a.tuple, vb, &b.tuple)
            || positional_union_is_exact(&a.tuple, a.tags, &b.tuple, b.tags, VECTOR_BIT))
        && (!contested(pa, &a.list_shape, pb, &b.list_shape)
            || positional_union_is_exact(&a.list_shape, a.tags, &b.list_shape, b.tags, PAIR_BIT))
    // The four literal slots are *never* contested: their union is the union of the
    // two literal sets, which `merge_union_lit_set` computes exactly.
}

/// The merged positional slot (a vector's `tuple`, a list's `list_shape` — `bit` says
/// which tag the slot refines) of `a ∪ b` when BOTH carry a shape of the same arity:
/// `Some(Some(shape))` with each position the union of the two — or `None` to fall back
/// to the plain rule (a side without a shape, or unequal arities). Exact when the shapes
/// differ in at most one position (`A×B ∪ C×B = (A∪C)×B`); a WIDENING otherwise — the
/// per-position union admits combinations neither side had — which is what the cap on a
/// union's terms reaches for: eight `[<literal> (+ i 1)]` branches of a parser used to
/// collapse to a bare `vector`, and every caller reading the index back got `any`.
fn positional_union(
    a: &Option<Arc<Vec<Ty>>>,
    a_tags: u32,
    b: &Option<Arc<Vec<Ty>>>,
    b_tags: u32,
    bit: u32,
) -> Option<Option<Arc<Vec<Ty>>>> {
    let (Some(ta), Some(tb)) = (a.as_deref(), b.as_deref()) else {
        return None;
    };
    if ta.len() != tb.len() || a_tags & bit == 0 || b_tags & bit == 0 {
        return None;
    }
    let merged: Vec<Ty> = ta
        .iter()
        .zip(tb)
        .map(|(x, y)| x.clone().union(y.clone()))
        .collect();
    Some(Some(Arc::new(merged)))
}

/// Is the positional half of `a ∪ b` EXACT under [`positional_union`] — same arity,
/// differing in at most one position (or identical)?
fn positional_union_is_exact(
    a: &Option<Arc<Vec<Ty>>>,
    a_tags: u32,
    b: &Option<Arc<Vec<Ty>>>,
    b_tags: u32,
    bit: u32,
) -> bool {
    let (Some(ta), Some(tb)) = (a.as_deref(), b.as_deref()) else {
        return false;
    };
    if ta.len() != tb.len() || a_tags & bit == 0 || b_tags & bit == 0 {
        return false;
    }
    // Exact when at most one position differs — or when one shape is inside the other
    // position-wise, so the union IS the wider shape: `(tuple m int)` beside `(tuple m
    // int[-1..])` is one tuple, not two alternatives for a merge to lose (ADR-350).
    ta.iter().zip(tb).filter(|(x, y)| x != y).count() <= 1
        || ta.iter().zip(tb).all(|(x, y)| x.is_subtype(y))
        || ta.iter().zip(tb).all(|(x, y)| y.is_subtype(x))
}
/// The merged element refinement of `a ∪ b` when that merge is EXACT although the two
/// element types differ — `Some(elem)` — or `None` when it is not (the caller then falls
/// back to the plain "identical or one-sided" rule, which widens).
///
/// For every sequence kind `s` (list, vector, set), `E₁ ⊆ E₂` gives `s<E₁> ⊆ s<E₂>`. So if
/// one side's element type is inside the other's AND that side's sequence kinds are among
/// the other's, the union of the two sequence parts is exactly the wider side's: a term
/// carrying the wider elements over the union of the tags denotes `⋃ s<E₂>` over the
/// wider side's kinds, which contains the narrower side's part and equals the wider's.
/// `nil | list<3>` ∪ `list<int>` is then `nil | list<int>` — one term — where before it was
/// two alternatives that no element read could speak for (a derivation only answers when
/// the term admits ONE collection), so a list built by a recursive accumulator degraded to
/// a bare `pair` on its second element. An absent refinement is "any element" and takes
/// part as such. Only for terms with no subtraction — `merge_is_exact` refuses those first.
fn elem_union_exact(a: &Ty, b: &Ty) -> Option<Option<Arc<Ty>>> {
    let (sa, sb) = (a.tags & SEQ_BITS, b.tags & SEQ_BITS);
    if sa == 0 || sb == 0 {
        return None; // one-sided: the plain rule is already exact
    }
    // A positional shape states its elements too — `[]` has NONE, so it sits inside any
    // `vector<T>` (the accumulator seed of every `conj` loop).
    let elem_of = |t: &Ty| {
        if let Some(e) = t.elem.as_deref() {
            return e.clone();
        }
        let shape = if t.tags & SEQ_BITS == VECTOR_BIT {
            t.tuple.as_deref()
        } else if t.tags & SEQ_BITS == PAIR_BIT {
            t.list_shape.as_deref()
        } else {
            None
        };
        match shape {
            Some(ts) => ts.iter().cloned().fold(Ty::NEVER, Ty::union),
            None => Ty::ANY,
        }
    };
    let (ea, eb) = (elem_of(a), elem_of(b));
    if ea == eb {
        return None; // identical: the plain rule keeps it
    }
    // The wider side's EFFECTIVE elements — carried as an element type when that side
    // stated them as a shape, since the shape itself does not survive the merge.
    let carry = |t: &Ty, e: Ty| -> Option<Arc<Ty>> {
        if t.elem.is_some() {
            t.elem.clone()
        } else if e == Ty::ANY {
            None
        } else {
            Some(Arc::new(e))
        }
    };
    if sa & !sb == 0 && ea.is_subtype(&eb) {
        return Some(carry(b, eb));
    }
    if sb & !sa == 0 && eb.is_subtype(&ea) {
        return Some(carry(a, ea));
    }
    None
}
/// The surviving refinement for a **union**: present on just one side → carry it;
/// on both and equal → keep; on both and different → widen to `None` (the union
/// of two distinct refinements isn't a single one). Shared by the `arrow` and
/// `elem` refinements (`present` is "does this side contribute the refined
/// members").
fn merge_union<T: PartialEq>(
    a_present: bool,
    a: &Option<Arc<T>>,
    b_present: bool,
    b: &Option<Arc<T>>,
) -> Option<Arc<T>> {
    match (a_present, b_present) {
        (true, false) => a.clone(),
        (false, true) => b.clone(),
        (true, true) if a == b => a.clone(),
        _ => None,
    }
}

/// The surviving refinement for an **intersection** (the relevant tag bit already
/// known to survive): the narrower of the two — a known refinement beats "any"
/// (`None`); two distinct known refinements widen to `None`.
fn merge_intersect<T: PartialEq>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> Option<Arc<T>> {
    match (a, b) {
        (Some(x), Some(y)) if x == y => Some(x.clone()),
        (Some(_), Some(_)) => None,
        (Some(x), None) => Some(x.clone()),
        (None, Some(y)) => Some(y.clone()),
        (None, None) => None,
    }
}

/// The surviving literal set for a **union** of one tag's literal member. Unlike
/// the generic [`merge_union`], two literal sets combine *exactly* (set-union) —
/// `{:a} ∪ {:b} = {:a, :b}`. But if either side has that member *open* (the tag
/// present with no literal set — i.e. "any keyword"/"any int"/…), the union
/// admits every value of the tag, so the result is open too (`None`). One
/// function over every literal kind (`Symbol`/`i64`/`bool`/`String`), each an
/// independent tag/field: pass the tag bit and both sides' `tags` + literal field.
fn merge_union_lit_set<T: Ord + Clone>(
    tag: u32,
    a_tags: u32,
    a: &Option<Arc<LitSet<T>>>,
    b_tags: u32,
    b: &Option<Arc<LitSet<T>>>,
) -> Option<Arc<LitSet<T>>> {
    // A side that carries the tag with no set admits *every* value of it, which absorbs
    // whatever the other side lists. A side that lacks the tag contributes nothing.
    let has = |tags: u32| tags & tag != 0;
    match (has(a_tags), has(b_tags)) {
        (false, false) => None,
        (true, false) => a.clone(),
        (false, true) => b.clone(),
        (true, true) => canon_lit(lit_union(a.as_deref(), b.as_deref()).map(Arc::new)),
    }
}

/// Terms sharing their exact tag set merged by the widening merge — except records,
/// which stay apart (see [`Ty::widen_intervals_against`]).
fn collapse_same_tags(terms: Vec<Ty>) -> Vec<Ty> {
    let mut out: Vec<Ty> = Vec::with_capacity(terms.len());
    'next: for term in terms {
        if term.fields.is_none() && !term.mu && !term.rec_ref && term.neg.is_none() {
            for existing in out.iter_mut() {
                if existing.tags & !NIL_BIT == term.tags & !NIL_BIT
                    && existing.fields.is_none()
                    && !existing.mu
                    && !existing.rec_ref
                    && existing.neg.is_none()
                    && !tagged_apart(existing, &term)
                {
                    // The widening merge — but a positional shape of one arity on both
                    // sides is kept as a shape, each position the hull of the two: an
                    // over-approximation of the union (sound) that a destructuring can
                    // still read, where the plain merge drops the shape the moment two
                    // positions differ (`[m idx]` beside `[(assoc m …) (dec n)]` read
                    // `int | map` for `m`).
                    let tuple = positional_hull(&existing.tuple, &term.tuple);
                    let list_shape = positional_hull(&existing.list_shape, &term.list_shape);
                    let mut merged = existing.clone().union_term(term);
                    if merged.tags & VECTOR_BIT != 0 && tuple.is_some() {
                        merged.tuple = tuple;
                    }
                    if merged.tags & PAIR_BIT != 0 && list_shape.is_some() {
                        merged.list_shape = list_shape;
                    }
                    *existing = merged.normalise_len();
                    continue 'next;
                }
            }
        }
        // Two record shapes over the SAME field names merge field-wise — the widening a
        // fold ascent needs when its accumulator is a record: the seed `{col: 1, ops:
        // (tuple)}` beside the step `{col: 2, ops: vector<string>[1]}` is one shape whose
        // `col` is then an interval on the move, not two alternatives that multiply by one
        // per round until the union caps. A shape with a different key set is left alone:
        // the tagged-union idiom (`{ok: …} | {error: …}`) keeps its arms.
        if let (Some(shape), false, false, None) = (&term.fields, term.mu, term.rec_ref, &term.neg)
        {
            for existing in out.iter_mut() {
                let Some(existing_shape) = &existing.fields else {
                    continue;
                };
                if existing.tags & !NIL_BIT == term.tags & !NIL_BIT
                    && !existing.mu
                    && !existing.rec_ref
                    && existing.neg.is_none()
                    && existing_shape.is_open() == shape.is_open()
                    && existing_shape.fields.keys().eq(shape.fields.keys())
                {
                    let fields = existing_shape
                        .fields
                        .iter()
                        .map(|(k, (t, req))| {
                            let (u, ureq) = &shape.fields[k];
                            (*k, (t.clone().union(u.clone()), *req && *ureq))
                        })
                        .collect();
                    let rest = existing_shape.rest.clone().union(shape.rest.clone());
                    let mut merged = existing.clone().union_term(term);
                    merged.fields = Some(Arc::new(RecordShape { fields, rest }));
                    *existing = merged;
                    continue 'next;
                }
            }
        }
        out.push(term);
    }
    out
}

/// The position-wise hull of two positional shapes of ONE arity — `Some` only when both
/// carry a shape and the arities agree. Every value of either shape is in the hull, so a
/// union widened to it is still over-approximated.
fn positional_hull(a: &Option<Arc<Vec<Ty>>>, b: &Option<Arc<Vec<Ty>>>) -> Option<Arc<Vec<Ty>>> {
    let (Some(ta), Some(tb)) = (a.as_deref(), b.as_deref()) else {
        return None;
    };
    if ta.len() != tb.len() {
        return None;
    }
    Some(Arc::new(
        ta.iter()
            .zip(tb.iter())
            .map(|(x, y)| x.clone().union(y.clone()))
            .collect(),
    ))
}

/// Are two positional shapes a TAGGED union — `[:ok v]` beside `[:error e]`, telling
/// apart by a keyword or string at some position? Those are the alternatives ADR-262
/// keeps, and a widening must not merge them; an accumulator's shapes differ in ints,
/// lengths and elements, never in a tag. The arities need not agree: `[:ok claims]`
/// beside `[:error status why]` is the same idiom, and merging the two by length hulled
/// them into a `vector[2..3]` whose second position was `502 | map` — a `match` on the
/// `:ok` tag could then tell nothing apart. Across arities only a KEYWORD counts as the
/// tag: a list builder's `(list "x")` beside `(list string "x")` differs by a string
/// literal at a position, and reading that as a tag kept every step of the ascent apart
/// (the accumulator never converged). Positions are compared as far as both go.
fn tagged_apart(a: &Ty, b: &Ty) -> bool {
    let (Some(ta), Some(tb)) = (
        a.tuple.as_deref().or(a.list_shape.as_deref()),
        b.tuple.as_deref().or(b.list_shape.as_deref()),
    ) else {
        return false;
    };
    let same_arity = ta.len() == tb.len();
    let is_tag = |t: &Ty| t.as_lit().is_some() || (same_arity && t.as_lit_str().is_some());
    ta.iter()
        .zip(tb.iter())
        .any(|(x, y)| x != y && (is_tag(x) || is_tag(y)))
}

/// The interval slot of a union: the HULL where both sides have the member, else the
/// side that has it. `None` for the unconstrained result.
fn merge_ranges(a_has: bool, a: Range, b_has: bool, b: Range) -> Option<Range> {
    let out = match (a_has, b_has) {
        (false, false) => return None,
        (true, false) => a,
        (false, true) => b,
        (true, true) => Range::hull(a, b),
    };
    (!out.is_all()).then_some(out)
}

/// The canonical int interval beside a literal set: a POSITIVE set says exactly which
/// ints, so an interval beside it is redundant; `ALL` is `None`.
fn canon_int_range(lit_int: &Option<Arc<LitSet<i64>>>, range: Option<Range>) -> Option<Range> {
    if matches!(lit_int.as_deref(), Some(LitSet::In(_))) {
        return None;
    }
    range.filter(|r| !r.is_all())
}

/// The canonical length beside the tags: none without a countable member, none for
/// `ALL`, and none for `[1, ∞)` on a term whose only countable member is `pair` (which
/// is at least 1 by being a pair).
fn canon_len(tags: u32, len: Option<Range>) -> Option<Range> {
    if tags & COUNT_BITS == 0 {
        return None;
    }
    // A length is never negative: an open lower end is 0.
    let len = len.map(|r| Range::new(Some(r.lo.unwrap_or(0).max(0)), r.hi));
    let len = len.filter(|r| !r.is_all() && *r != Range::at_least(0))?;
    if tags & COUNT_BITS == PAIR_BIT && len == Range::at_least(1) {
        return None;
    }
    Some(len)
}

/// A positive literal set restricted to an interval.
fn filter_lit_int(lit_int: Option<Arc<LitSet<i64>>>, r: Range) -> Option<Arc<LitSet<i64>>> {
    match lit_int.as_deref() {
        Some(LitSet::In(set)) => {
            let kept: BTreeSet<i64> = set
                .iter()
                .copied()
                .filter(|n| Range::subset(Range::point(*n), r))
                .collect();
            Some(Arc::new(LitSet::In(kept)))
        }
        _ => lit_int,
    }
}

/// The surviving literal set for an **intersection** of one tag's literal member
/// (the tag bit already known to survive): the narrower of the two — two sets
/// intersect exactly; an *open* side (no set) intersects to the other side's set.
/// The returned `bool` is `false` when the intersection is empty, so no value of
/// the tag qualifies and the caller clears the tag bit.
fn intersect_lit_set<T: Ord + Clone>(
    a: &Option<Arc<LitSet<T>>>,
    b: &Option<Arc<LitSet<T>>>,
) -> (Option<Arc<LitSet<T>>>, bool) {
    match lit_intersect(a.as_deref(), b.as_deref()) {
        // An empty positive set admits no value of the tag, so the tag itself drops.
        Some(set) if set.is_empty() => (None, false),
        Some(set) => (canon_lit(Some(Arc::new(set))), true),
        None => (None, true),
    }
}

fn lit_is_subtype<T: Ord>(
    self_has_tag: bool,
    a: &Option<Arc<LitSet<T>>>,
    b: &Option<Arc<LitSet<T>>>,
) -> bool {
    if !self_has_tag {
        return true;
    }
    lit_subset(a.as_deref(), b.as_deref())
}

/// Whether two literal sets decide **disjointness** for a tag that is the sole
/// shared tag: `Some(_)` when both sides pin a set (an exact enumeration), else
/// `None` (the caller falls through to its default). Only ever adds a
/// genuinely-disjoint verdict — advisory-soundness holds.
fn lit_disjoint<T: Ord>(
    shared_is_tag: bool,
    a: &Option<Arc<LitSet<T>>>,
    b: &Option<Arc<LitSet<T>>>,
) -> Option<bool> {
    if shared_is_tag && (a.is_some() || b.is_some()) {
        return Some(lit_sets_disjoint(a.as_deref(), b.as_deref()));
    }
    None
}

mod display;

/// A **gradual** type — `dynamic()` brought *inside* the lattice (ADR-024,
/// `docs/types.md`), not a bolt-on. It is a static [`Ty`] `bound` plus a
/// `dynamic` flag: flag clear → exactly the static set; flag set →
/// `dynamic(bound)`, "materialisable to anything within `bound`". Pure
/// `dynamic()` is `dynamic(ANY)`.
///
/// The defining property: **consistent subtyping is *derived from* set
/// inclusion**, never a separate consistency axiom (the classic Siek–Taha
/// bolt-on — see ADR-024). A value flows where a static `t` is expected iff a
/// static type does (`bound ⊆ t`) or — when dynamic — *some* inhabited
/// materialisation fits (`bound ∩ t ≠ ⊥`). So pure `dynamic()` is consistent with
/// every inhabited type (defer the check), while `dynamic(number)` is still
/// caught against `string`.
///
/// **The rule (no checker consumes it yet):** anything whose static type can't be
/// pinned — above all a *redefinable global under hot reload* — is `dynamic()`,
/// never `ANY`. (`ANY` relates by subtyping and would error where an `int` is
/// wanted; `dynamic()` defers, which is what lets typing coexist with live
/// redefinition.)
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GradualTy {
    /// What we statically know: every materialisation is `⊆ bound`.
    pub bound: Ty,
    /// Whether the gradual `?` is in play (materialisable within `bound`).
    pub dynamic: bool,
}

mod sig;
pub use sig::Sig;

/// The process-wide strict switch. `nest check --strict` flips it before it runs the
/// checker; with nothing set explicitly it is `BROOD_CHECK_STRICT=1`, read once — so
/// EVERY entry point (`brood --check`, the REPL's advisory check, the LSP,
/// `check-string-here`) honours the env spelling, not only the one command that used to
/// read it itself (2026-09-12: the flag catalogue and ADR-298 named it as the mode switch,
/// and `brood --check` with it set reported the plain verdict). A file check reads the
/// mode once, at its root (`check_file_ext`), into the checker context — so it is a
/// launch setting, never something the walk consults.
///
/// `0` = not set explicitly (the env decides), `1` = off, `2` = on.
static STRICT_CHECKING: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static STRICT_FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Turn strict checking on or off for this process — see
/// [`GradualTy::consistent_with_mode`]. An explicit setting wins over the env.
pub fn set_strict_checking(on: bool) {
    STRICT_CHECKING.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
}

/// Is strict checking on for this process?
pub fn strict_checking() -> bool {
    match STRICT_CHECKING.load(std::sync::atomic::Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => *STRICT_FROM_ENV
            .get_or_init(|| std::env::var_os("BROOD_CHECK_STRICT").is_some_and(|v| v == "1")),
    }
}

impl GradualTy {
    /// A purely static gradual type — exactly the set `t`, no `?`.
    pub const fn stat(t: Ty) -> GradualTy {
        GradualTy {
            bound: t,
            dynamic: false,
        }
    }

    /// `dynamic(bound)` — gradual, materialisable to anything within `bound`.
    pub const fn dynamic_within(bound: Ty) -> GradualTy {
        GradualTy {
            bound,
            dynamic: true,
        }
    }

    /// Pure `dynamic()` = `dynamic(ANY)` — the unknown type a redefinable global
    /// or free reference gets, so checking never fights hot reload.
    pub const fn dynamic() -> GradualTy {
        GradualTy::dynamic_within(Ty::ANY)
    }

    /// Is the gradual `?` in play?
    pub const fn is_dynamic(&self) -> bool {
        self.dynamic
    }

    /// **Consistent subtyping** into a static expectation — derived from set
    /// inclusion, the relation a checker uses for "can a value of this gradual
    /// type be used where `expected` is wanted?". Static: `bound ⊆ expected`.
    /// Dynamic: some inhabited materialisation fits, `bound ∩ expected ≠ ⊥`.
    pub fn consistent_with(&self, expected: Ty) -> bool {
        self.consistent_with_mode(expected, false)
    }

    /// [`consistent_with`](Self::consistent_with) with the **strict** switch (`nest check
    /// --strict`). Strict mode keeps the dynamic reading only for the genuinely unknown —
    /// a bare `dynamic()` (`any`) — and checks a dynamic value whose bound is anything
    /// narrower by inclusion, `bound ⊆ expected`, exactly like a static one. That is the
    /// "warn on the merely-wider" precision the gradual overlap rule deliberately gives
    /// up for reload-safety (docs/type-gating.md, B1): a `number` handed to an `int`
    /// parameter is *consistent* by overlap, and *rejected* strictly. Inclusion here is
    /// [`Ty::is_consistent_subtype`]: a NESTED unknown (a record field, an element) is
    /// the gradual `?` in either mode — only what is positively known is read strictly.
    pub fn consistent_with_mode(&self, expected: Ty, strict: bool) -> bool {
        if !self.dynamic {
            // A PRECISE bound — a literal, a sig-typed parameter — is read by inclusion, term
            // by term; but an arm known only by exclusion is the unknown even when it came
            // from a declaration: `(sig f (any -> …))` then `(when xs (first xs))` leaves
            // `xs : (not (nil | false))`, which says nothing positive, and reading it by
            // inclusion reported the guarded use of a declared-`any` parameter in both
            // modes. Declaring `any` means what not declaring means.
            return self.bound.terms_vec().into_iter().all(|term| {
                if term.is_known_only_by_exclusion() {
                    !term.is_disjoint(&expected)
                } else {
                    term.is_consistent_subtype(&expected)
                }
            });
        }
        // A dynamic bound is read TERM BY TERM. A union that puts a positively-known
        // alternative beside one known only by exclusion — `nil | list<int> | (not (nil |
        // vector))`, what a `cond` that returns its scrutinee in the `else` arm yields —
        // is not positively known as a whole, and reading it as one would charge the
        // known arms with the unknown arm's admissions: the exclusion arm admits a
        // `failure` the way it admits everything, and strict inclusion would demand that
        // "anything but nil or a vector" fit the parameter. So each arm gets the reading
        // its own knowledge earns, and the whole is consistent when every arm is.
        let failure = Ty::of(Tag::Failure);
        self.bound.terms_vec().into_iter().all(|term| {
            // Strict applies to a bound that is POSITIVELY known. `any ∖ nil` — what a
            // `(when x …)` guard leaves — says what the value is not, never what it is; it
            // is still the unknown, and reading it by inclusion would flag every guarded
            // use of an untyped parameter.
            let by_exclusion = term.is_known_only_by_exclusion();
            if strict && !by_exclusion {
                return term.is_consistent_subtype(&expected);
            }
            // **A failure is never a valid materialisation** of a domain that excludes
            // one — checked by inclusion in both modes, where every other arm of a union
            // gets the overlap reading. This is the converse of the impossible-`failure?`
            // lint (ADR-315): that one names a guard that cannot fire, this one names a
            // value that can fail where nothing guards it.
            //
            // Why `failure` and not, say, `nil`, when the merely-wider rule exists
            // precisely to keep the checker out of the way (docs/type-gating.md B1): the
            // two are different channels by construction (ADR-310). `nil` means
            // **nothing** and is a legitimate result everywhere — `(get m :absent)` is an
            // answer, and reading `(or T nil)` into `T` by inclusion would fire on every
            // accessor in the language. A **failure** means a value of the expected type
            // could not be produced; it is not an answer anybody's domain accepts, and a
            // failure reaching a primitive that cannot use it RAISES (ADR-315 kept that
            // deliberately). So this reports a guaranteed error on a reachable path, not
            // a merely-wider type.
            //
            // The failure arm has to be POSITIVELY known — it comes from a declared or
            // inferred signature, i.e. someone said the function can fail. A bound known
            // only by exclusion (`any`, or a guard's `(not nil)`) admits a failure the way
            // it admits everything, and says nothing; reading that as "can fail" would
            // fire on every unannotated parameter.
            //
            // Reload safety is unaffected: a `def` that stops returning failures changes
            // the signature the next check reads, and this warning goes with it.
            if !by_exclusion && !term.is_disjoint(&failure) && expected.is_disjoint(&failure) {
                return false;
            }
            // Some inhabited materialisation fits — i.e. the arm is not *provably
            // disjoint* from `expected`. Uses [`Ty::is_disjoint`], not
            // `intersect().is_never()`: the two agree on flat tags, but only
            // `is_disjoint` also sees the refinement-level conflicts (record
            // fields, tuple shapes, literal sets), so a dynamic value with a
            // refined type that provably can't fit is caught here too.
            !term.is_disjoint(&expected)
        })
    }

    /// Gradual union — union of bounds, dynamic if either side is. Used to join
    /// the types of branches (e.g. the arms of an `if`). The static set algebra
    /// lives on [`Ty`] (`self.bound`); the only gradual combinator we expose is
    /// the one a consumer needs — gradual intersection/negation are deferred
    /// until Step 4 shows their exact semantics (ADR-011: don't ship unproven
    /// operators).
    pub fn union(self, other: GradualTy) -> GradualTy {
        GradualTy {
            bound: self.bound.union(other.bound),
            dynamic: self.dynamic || other.dynamic,
        }
    }
}

#[cfg(test)]
mod tests;

/// [`Ty::widened_below`] over a signature's parameters and result.
fn widen_sig(sig: &Sig, depth: usize) -> Sig {
    let mut out = sig.clone();
    out.params = sig.params.iter().map(|t| t.widened_below(depth)).collect();
    out.optional = sig
        .optional
        .iter()
        .map(|t| t.widened_below(depth))
        .collect();
    out.rest = sig.rest.as_ref().map(|t| t.widened_below(depth));
    out.ret = sig.ret.widened_below(depth);
    out
}
