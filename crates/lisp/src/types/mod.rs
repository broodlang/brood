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

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use crate::core::value::{self, Symbol, Tag, Value};

/// Every tag, in bit order — for iterating a `Ty`'s members (printing, etc.) and
/// the source of [`TAG_COUNT`]. **Must list every [`Tag`] variant in discriminant
/// order**; the compiler can't enumerate variants, so `tag_universe_is_consistent`
/// (below) is what guards completeness, ordering, and the universe size.
const ALL_TAGS: [Tag; 22] = [
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
    Tag::Bitset,
    Tag::Bytes,
    Tag::Decimal,
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
const SEQ_BITS: u32 = (1u32 << bit(Tag::Pair)) | (1u32 << bit(Tag::Vector));

/// The map tag — the one tag a key/value refinement applies to.
const MAP_BIT: u32 = 1u32 << bit(Tag::Map);

/// The keyword tag — the one tag a literal (singleton) refinement applies to. A
/// keyword-literal type `:maximized` refines the keyword members to exactly the
/// listed keyword symbols (set-theoretic literal types, ADR; keyword-only first).
const KEYWORD_BIT: u32 = 1u32 << bit(Tag::Keyword);

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
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Ty {
    /// The set of possible runtime tags — always present; the coarse set.
    tags: u32,
    /// Refinement of the function members (`Fn`/`Native`), when statically known.
    /// `None` means "any function" (the permissive default).
    arrow: Option<Arc<Sig>>,
    /// Refinement of the sequence members (`pair`/`vector`) — the element type,
    /// when statically known. `None` means "elements of any type".
    elem: Option<Arc<Ty>>,
    /// Refinement of the map member (`map`) — `(key-type, val-type)`, when
    /// statically known.  `None` means "keys and values of any type".
    map_kv: Option<Arc<(Ty, Ty)>>,
    /// Refinement of the keyword member (`keyword`) to a literal set — the exact
    /// keyword symbols admitted, e.g. `{:maximized, :fullboth}`. `None` means "any
    /// keyword". When `Some`, the `Keyword` bit is in `tags` and the set is
    /// non-empty; the keyword member is constrained to the set while every *other*
    /// tag in `tags` stays open (so `(or :a :b nil)` admits the two keywords *and*
    /// `nil`). Unlike the other refinements, union of two literal sets is *exact*
    /// (the set-union), not a widening — so `(or :a :b)` keeps both.
    lit: Option<Arc<BTreeSet<Symbol>>>,
}

impl Ty {
    /// `⊥` — the empty set; the type of no value. A subtype of every type.
    pub const NEVER: Ty = Ty::flat(0);
    /// `⊤` — every tag; the type of any value. A supertype of every type.
    pub const ANY: Ty = Ty::flat(UNIVERSE);
    /// `int ∪ float ∪ decimal` — the named union the prelude's `number?` predicate
    /// implies. A `decimal` is a number (but not an integer).
    pub const NUMBER: Ty = Ty::flat(
        (1u32 << bit(Tag::Int)) | (1u32 << bit(Tag::Float)) | (1u32 << bit(Tag::Decimal)),
    );
    /// `nil ∪ pair` — the named union the prelude's `list?` predicate implies.
    pub const LIST: Ty = Ty::flat((1u32 << bit(Tag::Nil)) | (1u32 << bit(Tag::Pair)));

    /// A flat (unrefined) type from a raw tag bitset — the internal constructor
    /// every flat `Ty` funnels through. `const` so the named points above can be
    /// `const`; the set operations that combine refinements can't be.
    const fn flat(tags: u32) -> Ty {
        Ty {
            tags,
            arrow: None,
            elem: None,
            map_kv: None,
            lit: None,
        }
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
            tags: FN_BITS,
            arrow: Some(Arc::new(sig)),
            elem: None,
            map_kv: None,
            lit: None,
        }
    }

    /// The function-arrow refinement, if this type carries one. The bridge the
    /// advisory checker reads to compare a callback against what a higher-order
    /// function expects.
    pub fn as_arrow(&self) -> Option<&Sig> {
        self.arrow.as_deref()
    }

    /// A sequence type over `tags` (some subset of `pair`/`vector`) whose elements
    /// have type `elem` — the general element-refinement constructor.
    pub fn seq_of(tags: u32, elem: Ty) -> Ty {
        Ty {
            tags: tags & SEQ_BITS,
            arrow: None,
            elem: Some(Arc::new(elem)),
            map_kv: None,
            lit: None,
        }
    }

    /// `map<K, V>` — a map whose keys have type `K` and values have type `V`.
    pub fn map_of(key: Ty, val: Ty) -> Ty {
        Ty {
            tags: MAP_BIT,
            arrow: None,
            elem: None,
            map_kv: Some(Arc::new((key, val))),
            lit: None,
        }
    }

    /// A keyword-literal (singleton) type — exactly the keyword `sym`. Unions of
    /// these build an enumerated keyword type, e.g. `(or :maximized :fullboth)`.
    pub fn keyword_lit(sym: Symbol) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(sym);
        Ty {
            tags: KEYWORD_BIT,
            arrow: None,
            elem: None,
            map_kv: None,
            lit: Some(Arc::new(set)),
        }
    }

    /// The keyword-literal refinement, if this type carries one (the exact keyword
    /// symbols admitted). `None` means "any keyword" (or no keyword member).
    pub fn as_lit(&self) -> Option<&BTreeSet<Symbol>> {
        self.lit.as_deref()
    }

    /// The key/value refinement, if this map type carries one. The bridge the
    /// checker reads to flow `(get m k)` → `V | nil`, `(keys m)` → `list<K>`, etc.
    pub fn map_kv(&self) -> Option<(&Ty, &Ty)> {
        self.map_kv.as_deref().map(|(k, v)| (k, v))
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

    /// The element-type refinement, if this sequence type carries one. The bridge
    /// the checker reads to flow `(first xs)` / `(nth xs i)` to the element type.
    pub fn elem_ty(&self) -> Option<&Ty> {
        self.elem.as_deref()
    }

    /// The type of a concrete value — the bridge from a runtime value to its type.
    /// A keyword becomes its **literal singleton** (`:foo`, not the whole `keyword`
    /// tag), so a literal in code is checked against an enumerated keyword sig.
    pub fn of_value(v: Value) -> Ty {
        match v {
            Value::Keyword(s) => Ty::keyword_lit(s),
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
            "symbol?" => Ty::of(Tag::Sym),
            "keyword?" => Ty::of(Tag::Keyword),
            "string?" => Ty::of(Tag::Str),
            "pair?" => Ty::of(Tag::Pair),
            "vector?" => Ty::of(Tag::Vector),
            "map?" => Ty::of(Tag::Map),
            "ref?" => Ty::of(Tag::Ref),
            "pid?" => Ty::of(Tag::Pid),
            "rope?" => Ty::of(Tag::Rope),
            "socket?" => Ty::of(Tag::Socket),
            "subprocess?" => Ty::of(Tag::Subprocess),
            "table?" => Ty::of(Tag::Table),
            // `fn?` holds for both Brood closures and Rust builtins.
            "fn?" => Ty::of(Tag::Fn).union(Ty::of(Tag::Native)),
            "number?" => Ty::NUMBER,
            "list?" => Ty::LIST,
            _ => return None,
        })
    }

    /// `self ∪ other` — values in either. A refinement survives only where it's
    /// unambiguous: if just one side contributes the relevant members (functions
    /// for `arrow`, sequences for `elem`), that side's refinement carries; if both
    /// do, it survives only when they agree (the union of two distinct
    /// arrows/element-types isn't a single one → widen to "any"). Widening is
    /// sound: a union is a supertype anyway.
    pub fn union(self, other: Ty) -> Ty {
        let tags = self.tags | other.tags;
        let arrow = merge_union(
            self.tags & FN_BITS != 0,
            &self.arrow,
            other.tags & FN_BITS != 0,
            &other.arrow,
        );
        let elem = merge_union(
            self.tags & SEQ_BITS != 0,
            &self.elem,
            other.tags & SEQ_BITS != 0,
            &other.elem,
        );
        let map_kv = merge_union(
            self.tags & MAP_BIT != 0,
            &self.map_kv,
            other.tags & MAP_BIT != 0,
            &other.map_kv,
        );
        // Literal sets union *exactly* (not widen) — `:a ∪ :b = {a,b}`. But a side
        // whose keyword member is *open* (keyword tag, no literal set) contributes
        // every keyword, so the result keyword member is open too (`:a ∪ keyword =
        // keyword`).
        let lit = merge_union_lit(&self, &other);
        Ty {
            tags,
            arrow,
            elem,
            map_kv,
            lit,
        }
    }

    /// `self ∩ other` — values in both. When the relevant bit survives and one
    /// side is unrefined ("any"), the other side's refinement is the narrower —
    /// keep it; two distinct known refinements can't be one → widen. (Used by
    /// guard narrowing `T ∩ tested_by(pred)`, where `tested_by` is flat, so a
    /// refined `T` keeps its refinement through the narrow.)
    pub fn intersect(self, other: Ty) -> Ty {
        let mut tags = self.tags & other.tags;
        let arrow = if tags & FN_BITS != 0 {
            merge_intersect(&self.arrow, &other.arrow)
        } else {
            None
        };
        let elem = if tags & SEQ_BITS != 0 {
            merge_intersect(&self.elem, &other.elem)
        } else {
            None
        };
        let map_kv = if tags & MAP_BIT != 0 {
            merge_intersect(&self.map_kv, &other.map_kv)
        } else {
            None
        };
        // Literal sets intersect; if the result is empty no keyword qualifies, so
        // clear the keyword bit too. An *open* side (keyword, no set) intersects to
        // the other side's set (the narrower).
        let lit = if tags & KEYWORD_BIT != 0 {
            match (&self.lit, &other.lit) {
                (Some(a), Some(b)) => {
                    let s: BTreeSet<Symbol> = a.intersection(b).copied().collect();
                    if s.is_empty() {
                        tags &= !KEYWORD_BIT;
                        None
                    } else {
                        Some(Arc::new(s))
                    }
                }
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (None, None) => None,
            }
        } else {
            None
        };
        Ty {
            tags,
            arrow,
            elem,
            map_kv,
            lit,
        }
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
    pub fn negate(self) -> Ty {
        let mut tags = !self.tags & UNIVERSE;
        // A refinement means `self` omits some values of its refined tag(s);
        // those omitted values are in the complement, so the tag must survive.
        if self.arrow.is_some() {
            tags |= self.tags & FN_BITS;
        }
        if self.elem.is_some() {
            tags |= self.tags & SEQ_BITS;
        }
        if self.map_kv.is_some() {
            tags |= self.tags & MAP_BIT;
        }
        // A literal set omits the *other* keywords, which are in the complement —
        // so the keyword tag survives (widened to "any keyword").
        if self.lit.is_some() {
            tags |= KEYWORD_BIT;
        }
        Ty::flat(tags)
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
    pub fn is_subtype(&self, other: &Ty) -> bool {
        if self.tags & other.tags != self.tags {
            return false;
        }
        if self.tags & FN_BITS != 0 {
            if let Some(b) = &other.arrow {
                match &self.arrow {
                    Some(a) => {
                        if !a.is_subtype(b) {
                            return false;
                        }
                    }
                    None => return false, // self = "any function" ⊄ a specific arrow
                }
            }
        }
        if self.tags & SEQ_BITS != 0 {
            if let Some(b) = &other.elem {
                match &self.elem {
                    Some(a) => {
                        if !a.is_subtype(b) {
                            return false;
                        }
                    }
                    None => return false, // self = "any elements" ⊄ a specific elem
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
                    None => return false, // self = "any map" ⊄ a specific map<K,V>
                }
            }
        }
        if self.tags & KEYWORD_BIT != 0 {
            if let Some(b) = &other.lit {
                match &self.lit {
                    // every keyword self admits must be one `other` admits
                    Some(a) => {
                        if !a.is_subset(b) {
                            return false;
                        }
                    }
                    None => return false, // self = "any keyword" ⊄ a literal set
                }
            }
        }
        true
    }

    /// Do `self` and `other` share no values? (`self ∩ other = ⊥`.) Tag overlap
    /// decides it, with one *precise* exception: when the only shared tag is
    /// `keyword` and both sides pin disjoint literal sets, no keyword satisfies
    /// both. This only ever *adds* genuinely-disjoint cases (a literal set is an
    /// exact enumeration, not an approximation), so it can't raise a false warning
    /// — advisory-soundness holds.
    pub fn is_disjoint(&self, other: &Ty) -> bool {
        let shared = self.tags & other.tags;
        if shared == 0 {
            return true;
        }
        if shared == KEYWORD_BIT {
            if let (Some(a), Some(b)) = (&self.lit, &other.lit) {
                return a.is_disjoint(b);
            }
        }
        false
    }

    /// Does this type admit a value with `tag`?
    pub const fn contains_tag(&self, tag: Tag) -> bool {
        self.tags & (1u32 << bit(tag)) != 0
    }

    /// Is this the empty type `⊥` (no value inhabits it)?
    pub const fn is_never(&self) -> bool {
        self.tags == 0
    }

    /// Is this the universe `⊤` (every value inhabits it)?
    pub const fn is_any(&self) -> bool {
        self.tags == UNIVERSE
    }
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

/// The surviving keyword-literal set for a **union**. Unlike the generic
/// [`merge_union`], two literal sets combine *exactly* (set-union), since the union
/// of `{:a}` and `{:b}` is precisely `{:a, :b}`. But if either side has its keyword
/// member *open* (the keyword tag present with no literal set — i.e. "any keyword"),
/// the union admits every keyword, so the result is open too (`None`).
fn merge_union_lit(a: &Ty, b: &Ty) -> Option<Arc<BTreeSet<Symbol>>> {
    let open = |t: &Ty| t.tags & KEYWORD_BIT != 0 && t.lit.is_none();
    if open(a) || open(b) {
        return None;
    }
    match (&a.lit, &b.lit) {
        (None, None) => None,
        (x, y) => {
            let mut set = BTreeSet::new();
            if let Some(x) = x {
                set.extend(x.iter().copied());
            }
            if let Some(y) = y {
                set.extend(y.iter().copied());
            }
            if set.is_empty() {
                None
            } else {
                Some(Arc::new(set))
            }
        }
    }
}

impl fmt::Display for Ty {
    /// A readable rendering for diagnostics: the named lattice points where they
    /// apply (`never`, `any`, `number`, `list`), a single tag by its `type-of`
    /// name, otherwise the members joined with ` | ` (e.g. `int | string`). A
    /// purely-function type with a known arrow renders as `(p1, p2) -> ret`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Named points (compared by value — `Arc` isn't structural, so these
        // can't be `match` patterns).
        if *self == Ty::NEVER {
            return f.write_str("never");
        }
        if *self == Ty::ANY {
            return f.write_str("any");
        }
        if *self == Ty::NUMBER {
            return f.write_str("number");
        }
        if *self == Ty::LIST {
            return f.write_str("list");
        }
        // A purely-function type with a known signature: show the arrow.
        if self.tags & !FN_BITS == 0 {
            if let Some(sig) = self.as_arrow() {
                return write!(f, "{sig}");
            }
        }
        // A pure sequence type with a known element type: `vector<E>` / `list<E>`
        // — with a leading `nil | ` when the empty/empty-list case rides along
        // (e.g. a `(map …)` result is `nil | list<E>`), so the rendering names
        // every tag the value can actually have.
        // A pure map type with a known key/value type: `map<K, V>`.
        if let Some((k, v)) = self.map_kv() {
            if self.tags == MAP_BIT {
                return write!(f, "map<{k}, {v}>");
            }
        }
        if let Some(elem) = self.elem_ty() {
            if self.tags & !(SEQ_BITS | (1u32 << bit(Tag::Nil))) == 0 {
                let has_vec = self.contains_tag(Tag::Vector);
                let has_pair = self.contains_tag(Tag::Pair);
                let nil = if self.contains_tag(Tag::Nil) && (has_vec || has_pair) {
                    "nil | "
                } else {
                    ""
                };
                if has_vec && !has_pair {
                    return write!(f, "{nil}vector<{elem}>");
                }
                if has_pair && !has_vec {
                    return write!(f, "{nil}list<{elem}>");
                }
                if has_vec && has_pair {
                    return write!(f, "{nil}(list | vector)<{elem}>");
                }
            }
        }
        // A keyword-literal type: the enumerated keywords (`:a | :b`), plus any
        // other tags this type also admits (`:a | :b | nil`). Sorted by name so the
        // rendering is stable regardless of intern order.
        if let Some(set) = &self.lit {
            let mut parts: Vec<String> = set
                .iter()
                .map(|s| format!(":{}", value::symbol_name_ref(*s)))
                .collect();
            parts.sort();
            for tag in ALL_TAGS {
                if tag as u8 as u32 != bit(Tag::Keyword) && self.contains_tag(tag) {
                    parts.push(tag.name().to_string());
                }
            }
            return f.write_str(&parts.join(" | "));
        }
        let mut first = true;
        for tag in ALL_TAGS {
            if self.contains_tag(tag) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(tag.name())?;
            }
        }
        Ok(())
    }
}

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

/// A function's type signature: the static type of each fixed positional
/// argument, an optional type for the variadic tail (`rest`), and the result
/// type. The advisory checker (see [`check`]) reads this to decide whether a
/// call's arguments are provably wrong.
///
/// **Carried on every primitive [`NativeFn`](crate::core::value::NativeFn) —
/// the enforcement of compatibility-contract point #6:** adding a new
/// primitive without a signature is a compile error. Closures don't carry one
/// (yet); for the narrow set the checker can handle, [`check`] *infers* a
/// `Sig` from a straight-line one-expression body.
///
/// `params` is a [`Vec<Ty>`] (not `&'static [Ty]`) so the same type works for
/// inferred closure sigs built at check time, not just for static primitive
/// declarations.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Sig {
    /// The fixed positional argument types, in order.
    pub params: Vec<Ty>,
    /// The variadic-tail type — applies to every argument beyond `params`.
    /// `None` means no rest (extras are an arity error, caught separately).
    pub rest: Option<Ty>,
    /// The result type.
    pub ret: Ty,
}

impl Sig {
    /// `params -> ret` — fixed arity, no rest tail.
    pub fn new(params: Vec<Ty>, ret: Ty) -> Sig {
        Sig {
            params,
            rest: None,
            ret,
        }
    }
    /// `() -> ret` — a nullary primitive (a thunk / accessor).
    pub fn nullary(ret: Ty) -> Sig {
        Sig {
            params: Vec::new(),
            rest: None,
            ret,
        }
    }
    /// `(...rest) -> ret` — pure variadic, every argument is `rest`.
    pub fn variadic(rest: Ty, ret: Ty) -> Sig {
        Sig {
            params: Vec::new(),
            rest: Some(rest),
            ret,
        }
    }
    /// `params... ...rest -> ret` — fixed leading params then a variadic tail.
    pub fn with_rest(params: Vec<Ty>, rest: Ty, ret: Ty) -> Sig {
        Sig {
            params,
            rest: Some(rest),
            ret,
        }
    }
    /// `(...any) -> any` — the catch-all when a primitive's args/result aren't
    /// usefully pinned. The checker's disjointness test never warns against
    /// `ANY` (it overlaps every inhabited type), so this reads exactly like
    /// "no useful signature" while still satisfying contract point #6.
    pub fn any() -> Sig {
        Sig::variadic(Ty::ANY, Ty::ANY)
    }
    /// The type expected at argument position `i` — fixed params first, then
    /// `rest` for anything beyond. `None` when too many args are passed for
    /// a non-variadic sig (a separate arity check catches that).
    pub fn param(&self, i: usize) -> Option<Ty> {
        self.params.get(i).cloned().or_else(|| self.rest.clone())
    }

    /// Arrow subtyping `self <: other` — a function of type `self` is usable
    /// wherever `other` is expected. **Contravariant in parameters** (`self` must
    /// accept everything `other` might pass: `other.param(i) <: self.param(i)`)
    /// and **covariant in the result** (`self.ret <: other.ret`). Arities must
    /// be compatible. Used by [`Ty::is_subtype`] for the function members and by
    /// the checker's callback compatibility step.
    pub fn is_subtype(&self, other: &Sig) -> bool {
        // Result: covariant.
        if !self.ret.is_subtype(&other.ret) {
            return false;
        }
        // Arity must line up: a fixed-arity `self` can't satisfy an `other` that
        // may pass more (or fewer) arguments than `self` accepts.
        match (self.rest.is_some(), other.rest.is_some()) {
            (false, true) => return false, // other is variadic, self isn't
            (false, false) if self.params.len() != other.params.len() => return false,
            // The remaining cases — `(true, _)`: a variadic `self` — are not
            // rejected here; their arity compatibility is checked positionally by
            // the param loop below, which iterates max(len) positions and uses
            // `param(i)` (folding `rest` in), so a variadic `self` is required to
            // accept every argument `other` may supply.
            _ => {}
        }
        // Parameters: contravariant — for every position `other` may supply,
        // `self` must accept at least as much.
        let arity = self.params.len().max(other.params.len());
        for i in 0..arity {
            match (other.param(i), self.param(i)) {
                (Some(o), Some(s)) => {
                    if !o.is_subtype(&s) {
                        return false;
                    }
                }
                // `other` supplies an argument `self` has no parameter for.
                (Some(_), None) => return false,
                _ => {}
            }
        }
        true
    }
}

impl fmt::Display for Sig {
    /// `(p1, p2) -> ret`, with a trailing `...rest` for the variadic tail and
    /// `()` for nullary — the arrow rendering used in diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        let mut first = true;
        for p in &self.params {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{p}")?;
        }
        if let Some(rest) = &self.rest {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "...{rest}")?;
        }
        write!(f, ") -> {}", self.ret)
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
        if self.dynamic {
            !self.bound.clone().intersect(expected).is_never()
        } else {
            self.bound.is_subtype(&expected)
        }
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
mod tests {
    use super::*;
    use crate::core::value::Value;

    #[test]
    fn singletons_and_named_unions() {
        assert_eq!(
            Ty::NUMBER,
            Ty::of(Tag::Int)
                .union(Ty::of(Tag::Float))
                .union(Ty::of(Tag::Decimal))
        );
        assert_eq!(Ty::LIST, Ty::of(Tag::Nil).union(Ty::of(Tag::Pair)));
        assert!(Ty::of(Tag::Int).contains_tag(Tag::Int));
        assert!(!Ty::of(Tag::Int).contains_tag(Tag::Float));
    }

    #[test]
    fn subtyping_is_set_inclusion() {
        assert!(Ty::of(Tag::Int).is_subtype(&Ty::NUMBER)); // int ⊆ number
        assert!(Ty::NUMBER.is_subtype(&Ty::ANY)); // number ⊆ any
        assert!(!Ty::NUMBER.is_subtype(&Ty::of(Tag::Int))); // number ⊄ int
                                                            // ⊥ is a subtype of everything; everything is a subtype of ⊤.
        assert!(Ty::NEVER.is_subtype(&Ty::of(Tag::Str)));
        assert!(Ty::of(Tag::Str).is_subtype(&Ty::ANY));
        assert!(Ty::of(Tag::Int).is_subtype(&Ty::of(Tag::Int))); // reflexive
    }

    #[test]
    fn intersection_and_disjointness() {
        assert_eq!(Ty::NUMBER.intersect(Ty::of(Tag::Int)), Ty::of(Tag::Int));
        assert_eq!(Ty::NUMBER.intersect(Ty::of(Tag::Str)), Ty::NEVER);
        assert!(Ty::NUMBER.is_disjoint(&Ty::LIST));
        assert!(!Ty::NUMBER.is_disjoint(&Ty::of(Tag::Float)));
    }

    #[test]
    fn negation_and_difference() {
        assert_eq!(Ty::NEVER.negate(), Ty::ANY);
        assert_eq!(Ty::ANY.negate(), Ty::NEVER);
        let not_nil = Ty::of(Tag::Nil).negate();
        assert!(!not_nil.contains_tag(Tag::Nil));
        assert!(not_nil.contains_tag(Tag::Int));
        // number \ int = float ∪ decimal
        assert_eq!(
            Ty::NUMBER.difference(Ty::of(Tag::Int)),
            Ty::of(Tag::Float).union(Ty::of(Tag::Decimal))
        );
    }

    #[test]
    fn of_value_bridges_runtime_values() {
        // These Value variants are heap-free, so no Heap is needed.
        assert_eq!(Ty::of_value(Value::int(1)), Ty::of(Tag::Int));
        assert_eq!(Ty::of_value(Value::nil()), Ty::of(Tag::Nil));
        assert_eq!(Ty::of_value(Value::boolean(true)), Ty::of(Tag::Bool));
        assert!(Ty::of_value(Value::int(1)).is_subtype(&Ty::NUMBER));
    }

    #[test]
    fn display_is_readable() {
        assert_eq!(Ty::NEVER.to_string(), "never");
        assert_eq!(Ty::ANY.to_string(), "any");
        assert_eq!(Ty::NUMBER.to_string(), "number");
        assert_eq!(Ty::LIST.to_string(), "list");
        assert_eq!(Ty::of(Tag::Int).to_string(), "int");
        assert_eq!(
            Ty::of(Tag::Int).union(Ty::of(Tag::Str)).to_string(),
            "int | string"
        );
    }

    #[test]
    fn tested_by_maps_predicates_to_the_type_they_assert() {
        assert_eq!(Ty::tested_by("int?"), Some(Ty::of(Tag::Int)));
        assert_eq!(Ty::tested_by("number?"), Some(Ty::NUMBER));
        assert_eq!(Ty::tested_by("list?"), Some(Ty::LIST));
        assert_eq!(Ty::tested_by("nil?"), Some(Ty::of(Tag::Nil)));
        // fn? covers Brood closures and Rust builtins both.
        assert_eq!(
            Ty::tested_by("fn?"),
            Some(Ty::of(Tag::Fn).union(Ty::of(Tag::Native)))
        );
        // Non-tag predicates and unknown names don't narrow.
        assert_eq!(Ty::tested_by("empty?"), None);
        assert_eq!(Ty::tested_by("zero?"), None);
        assert_eq!(Ty::tested_by("frobnicate?"), None);
    }

    #[test]
    fn single_tag_display_matches_tag_name() {
        // Contract point #9: a singleton Ty prints as its `type-of` / `Tag::name`
        // spelling, so a type named in a message reads the same as `type-of`
        // returns. (Locks errors / type-of / Ty against name drift.)
        for tag in ALL_TAGS {
            assert_eq!(Ty::of(tag).to_string(), tag.name());
        }
    }

    #[test]
    fn tag_universe_is_consistent() {
        // Guards contract point #1: the bits, ALL_TAGS, and the universe size all
        // agree. `bit` is the `#[repr(u8)]` discriminant, so this also catches a
        // tag missing from (or misordered in) ALL_TAGS — the gap a plain
        // exhaustive match can't, since Rust can't enumerate enum variants.
        for (i, tag) in ALL_TAGS.iter().enumerate() {
            // ALL_TAGS is in discriminant/bit order, densely from 0.
            assert_eq!(
                bit(*tag),
                i as u32,
                "{} is out of order in ALL_TAGS",
                tag.name()
            );
            // Every atom's bit is inside the universe...
            assert!(bit(*tag) < TAG_COUNT);
            // ...so every singleton is a subtype of ANY (none falls outside ⊤).
            assert!(Ty::of(*tag).is_subtype(&Ty::ANY));
        }
        assert_eq!(
            UNIVERSE.count_ones(),
            TAG_COUNT,
            "universe must cover every atom"
        );
    }

    #[test]
    fn pure_dynamic_is_consistent_with_every_inhabited_type() {
        let d = GradualTy::dynamic();
        assert!(d.is_dynamic());
        for t in [
            Ty::of(Tag::Int),
            Ty::NUMBER,
            Ty::of(Tag::Str),
            Ty::LIST,
            Ty::ANY,
        ] {
            assert!(
                d.consistent_with(t.clone()),
                "dynamic() should be consistent with {t}"
            );
        }
    }

    #[test]
    fn bounded_dynamic_still_discriminates() {
        // dynamic(number) defers within numbers but is still caught against string.
        let dnum = GradualTy::dynamic_within(Ty::NUMBER);
        assert!(dnum.consistent_with(Ty::of(Tag::Int)));
        assert!(dnum.consistent_with(Ty::of(Tag::Float)));
        assert!(!dnum.consistent_with(Ty::of(Tag::Str)));
    }

    #[test]
    fn static_gradual_is_plain_subtyping() {
        // Flag clear → consistent_with is exactly set inclusion.
        assert!(GradualTy::stat(Ty::of(Tag::Int)).consistent_with(Ty::NUMBER));
        assert!(!GradualTy::stat(Ty::NUMBER).consistent_with(Ty::of(Tag::Int)));
    }

    #[test]
    fn composes_with_set_operations() {
        let g =
            GradualTy::dynamic_within(Ty::of(Tag::Int)).union(GradualTy::stat(Ty::of(Tag::Str)));
        assert_eq!(g.bound, Ty::of(Tag::Int).union(Ty::of(Tag::Str)));
        assert!(g.is_dynamic()); // dynamic propagates through the union
    }

    #[test]
    fn static_union_stays_static() {
        let g = GradualTy::stat(Ty::of(Tag::Int)).union(GradualTy::stat(Ty::of(Tag::Str)));
        assert!(!g.is_dynamic());
    }

    #[test]
    fn dynamic_vs_never_is_the_degenerate_case() {
        // Nothing inhabits NEVER, so even dynamic() can't be used there...
        assert!(!GradualTy::dynamic().consistent_with(Ty::NEVER));
        // ...while a *static* NEVER (⊥) is a subtype of every type.
        assert!(GradualTy::stat(Ty::NEVER).consistent_with(Ty::of(Tag::Int)));
    }

    // ---- the set algebra obeys the lattice laws, over a representative sample ----

    // Deliberately **flat** types only — no refined (element-typed / arrow)
    // types. `negate` widens a refinement (see `Ty::negate`, the doc at
    // ~line 291), so double-negation and De Morgan are exact *only* for flat
    // types and would fail here for a refined one. That widening is intentional
    // (advisory soundness), so it's excluded from the laws and pinned on its own
    // in `negate_of_a_refined_type_is_a_sound_overapproximation` /
    // `double_negation_widens_a_refined_type`.
    fn sample_tys() -> Vec<Ty> {
        let mut v = vec![Ty::NEVER, Ty::ANY, Ty::NUMBER, Ty::LIST];
        for t in ALL_TAGS {
            v.push(Ty::of(t));
        }
        v.push(Ty::of(Tag::Int).union(Ty::of(Tag::Str)));
        v.push(Ty::NUMBER.union(Ty::of(Tag::Nil)));
        v
    }

    #[test]
    fn lattice_laws_hold() {
        // `Ty` is no longer `Copy` (the arrow refinement), so the by-value set
        // ops `.clone()` their operands here; the sample is all flat, so this is
        // exactly the pre-Step-5 algebra.
        let s = sample_tys();
        for a in &s {
            assert_eq!(a.clone().union(Ty::NEVER), *a, "∪⊥ identity");
            assert_eq!(a.clone().intersect(Ty::ANY), *a, "∩⊤ identity");
            assert_eq!(a.clone().union(a.clone()), *a, "∪ idempotent");
            assert_eq!(a.clone().intersect(a.clone()), *a, "∩ idempotent");
            assert_eq!(a.clone().union(a.clone().negate()), Ty::ANY, "complement ∪");
            assert_eq!(
                a.clone().intersect(a.clone().negate()),
                Ty::NEVER,
                "complement ∩"
            );
            assert_eq!(a.clone().negate().negate(), *a, "double negation");
            for b in &s {
                assert_eq!(
                    a.clone().union(b.clone()),
                    b.clone().union(a.clone()),
                    "∪ commutes"
                );
                assert_eq!(
                    a.clone().intersect(b.clone()),
                    b.clone().intersect(a.clone()),
                    "∩ commutes"
                );
                // subtyping IS set inclusion: a ⊆ b ⟺ a ∩ b = a
                assert_eq!(
                    a.is_subtype(b),
                    a.clone().intersect(b.clone()) == *a,
                    "subtype ⟺ inclusion"
                );
                // disjoint IS empty intersection
                assert_eq!(
                    a.is_disjoint(b),
                    a.clone().intersect(b.clone()).is_never(),
                    "disjoint ⟺ ∅"
                );
                // De Morgan
                assert_eq!(
                    a.clone().union(b.clone()).negate(),
                    a.clone().negate().intersect(b.clone().negate()),
                    "De Morgan"
                );
            }
        }
    }

    #[test]
    fn subtyping_is_reflexive_and_transitive() {
        let s = sample_tys();
        for a in &s {
            assert!(a.is_subtype(a));
            for b in &s {
                for c in &s {
                    if a.is_subtype(b) && b.is_subtype(c) {
                        assert!(a.is_subtype(c), "subtype transitivity");
                    }
                }
            }
        }
    }

    // ---- structured (arrow) types — Step 5+, ADR-078 ----

    fn arr(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::arrow(Sig::new(params, ret))
    }

    #[test]
    fn arrow_renders_as_an_arrow() {
        assert_eq!(
            arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int)).to_string(),
            "(int) -> int"
        );
        assert_eq!(
            arr(vec![Ty::of(Tag::Int), Ty::of(Tag::Str)], Ty::NUMBER).to_string(),
            "(int, string) -> number"
        );
        // A bare "any function" (no refinement) still prints as its tags.
        assert_eq!(
            Ty::of_tags(&[Tag::Fn, Tag::Native]).to_string(),
            "fn | native"
        );
    }

    #[test]
    fn arrow_subtyping_is_contravariant_then_covariant() {
        // (number) -> int  <:  (int) -> number
        //   params contravariant: int ⊆ number ✓     result covariant: int ⊆ number ✓
        let wide_in_narrow_out = arr(vec![Ty::NUMBER], Ty::of(Tag::Int));
        let narrow_in_wide_out = arr(vec![Ty::of(Tag::Int)], Ty::NUMBER);
        assert!(wide_in_narrow_out.is_subtype(&narrow_in_wide_out));
        assert!(!narrow_in_wide_out.is_subtype(&wide_in_narrow_out));
        // an unrefined "any function" is not a subtype of a specific arrow
        let any_fn = Ty::of_tags(&[Tag::Fn, Tag::Native]);
        assert!(!any_fn.is_subtype(&narrow_in_wide_out));
        // ...but a specific arrow *is* a subtype of "any function"
        assert!(narrow_in_wide_out.is_subtype(&any_fn));
    }

    #[test]
    fn arrow_arity_matters_for_subtyping() {
        let unary = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
        let binary = arr(vec![Ty::of(Tag::Int), Ty::of(Tag::Int)], Ty::of(Tag::Int));
        assert!(!unary.is_subtype(&binary));
        assert!(!binary.is_subtype(&unary));
    }

    #[test]
    fn union_keeps_a_lone_arrow_but_widens_two() {
        let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
        let g = arr(vec![Ty::of(Tag::Str)], Ty::of(Tag::Str));
        // int ∪ (int -> int): only one side contributes functions → arrow survives.
        let mixed = Ty::of(Tag::Int).union(f.clone());
        assert!(mixed.contains_tag(Tag::Int));
        assert_eq!(mixed.as_arrow(), f.as_arrow());
        // two distinct arrows can't be one arrow → widen to "any function".
        let widened = f.clone().union(g);
        assert!(widened.contains_tag(Tag::Fn));
        assert_eq!(widened.as_arrow(), None);
    }

    #[test]
    fn intersect_narrows_to_the_known_arrow() {
        let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
        let any_fn = Ty::of_tags(&[Tag::Fn, Tag::Native]); // unrefined
                                                           // refined ∩ any-function → keep the refinement (narrowing via fn? guard).
        assert_eq!(f.clone().intersect(any_fn).as_arrow(), f.as_arrow());
    }

    #[test]
    fn disjointness_ignores_arrow_mismatch() {
        // Two incompatible arrows are still both functions — NOT disjoint, so the
        // advisory checker never raises a false positive off an arrow mismatch.
        let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
        let g = arr(vec![Ty::of(Tag::Str)], Ty::of(Tag::Str));
        assert!(!f.is_disjoint(&g));
        // a function and a non-function are disjoint (tags don't overlap).
        assert!(f.is_disjoint(&Ty::of(Tag::Int)));
    }

    // ---- structured (element) types — Step 5+, ADR-078 slice 2 ----

    #[test]
    fn sequence_types_render_with_element() {
        assert_eq!(Ty::vector_of(Ty::of(Tag::Int)).to_string(), "vector<int>");
        assert_eq!(Ty::list_of(Ty::NUMBER).to_string(), "list<number>");
        assert_eq!(
            Ty::vector_of(Ty::of(Tag::Int).union(Ty::of(Tag::Str))).to_string(),
            "vector<int | string>"
        );
        // a bare vector (no element refinement) still prints as its tag
        assert_eq!(Ty::of(Tag::Vector).to_string(), "vector");
        // `nil | list<E>` (the shape a `(map …)`/`(filter …)` result carries)
        // names the nil rather than hiding it.
        assert_eq!(
            Ty::list_of(Ty::of(Tag::Int))
                .union(Ty::of(Tag::Nil))
                .to_string(),
            "nil | list<int>"
        );
    }

    #[test]
    fn negate_of_a_refined_type_is_a_sound_overapproximation() {
        // ¬(vector<int>) must be a *superset* of the true complement, so it has
        // to KEEP the `vector` tag — vectors holding a non-int element are in the
        // complement. The earlier impl dropped the tag (a subset), which could
        // manufacture a false `is_disjoint`.
        let nvi = Ty::vector_of(Ty::of(Tag::Int)).negate();
        assert!(nvi.contains_tag(Tag::Vector), "must keep the refined tag");
        // ...so it is NOT disjoint from another vector type — no false positive.
        assert!(!nvi.is_disjoint(&Ty::vector_of(Ty::of(Tag::Str))));
        assert!(!nvi.is_disjoint(&Ty::of(Tag::Vector)));
        // and it still admits the obviously-complement tags.
        assert!(nvi.contains_tag(Tag::Int));
        // Same widening for an arrow refinement: keep both function tags.
        let narr = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int)).negate();
        assert!(narr.contains_tag(Tag::Fn) && narr.contains_tag(Tag::Native));
        // Flat negate is unchanged (exact): ¬int still excludes int.
        assert!(!Ty::of(Tag::Int).negate().contains_tag(Tag::Int));
    }

    #[test]
    fn double_negation_widens_a_refined_type() {
        // Pins the documented exception the lattice-laws test deliberately can't
        // exercise (its `sample_tys` is flat-only): for a *refined* type the
        // widening in `negate` means double-negation does NOT round-trip.
        //
        // ¬(vector<int>) keeps the `vector` tag (a vector of non-ints is in the
        // complement) and adds every non-vector tag → that's `any`. ¬any = never.
        // So ¬¬(vector<int>) == never, neither the original nor a bare `vector`.
        let vi = Ty::vector_of(Ty::of(Tag::Int));
        let once = vi.clone().negate();
        assert_eq!(once, Ty::ANY, "¬(vector<int>) widens all the way to any");
        assert_eq!(once.negate(), Ty::NEVER, "…so ¬¬ collapses to never");
        assert_ne!(
            vi.clone().negate().negate(),
            vi,
            "double negation does NOT hold"
        );
        // The same collapse for an arrow refinement: ¬¬((int)->int) == never.
        let ai = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
        assert_eq!(ai.clone().negate(), Ty::ANY);
        assert_eq!(ai.negate().negate(), Ty::NEVER);
    }

    #[test]
    fn element_type_is_covariant_under_subtyping() {
        // vector<int> <: vector<number>  (int ⊆ number; immutable seqs are covariant)
        assert!(Ty::vector_of(Ty::of(Tag::Int)).is_subtype(&Ty::vector_of(Ty::NUMBER)));
        assert!(!Ty::vector_of(Ty::NUMBER).is_subtype(&Ty::vector_of(Ty::of(Tag::Int))));
        // a specific element type <: an unrefined vector ("any elements")
        assert!(Ty::vector_of(Ty::of(Tag::Int)).is_subtype(&Ty::of(Tag::Vector)));
        // ...but "any elements" is NOT a subtype of a specific element type
        assert!(!Ty::of(Tag::Vector).is_subtype(&Ty::vector_of(Ty::of(Tag::Int))));
        // different containers don't subtype (tags differ)
        assert!(!Ty::vector_of(Ty::of(Tag::Int)).is_subtype(&Ty::list_of(Ty::of(Tag::Int))));
    }

    #[test]
    fn element_refinement_widens_on_a_union_mismatch_but_keeps_a_match() {
        let vi = Ty::vector_of(Ty::of(Tag::Int));
        let vs = Ty::vector_of(Ty::of(Tag::Str));
        // vector<int> ∪ vector<string> → vector (element widened; sound supertype)
        let u = vi.clone().union(vs);
        assert!(u.contains_tag(Tag::Vector));
        assert_eq!(u.elem_ty(), None);
        // vector<int> ∪ vector<int> → vector<int> (agree → kept)
        assert_eq!(vi.clone().union(vi.clone()).elem_ty(), vi.elem_ty());
        // int ∪ vector<int> → only the vector side contributes elements → kept
        let mixed = Ty::of(Tag::Int).union(vi.clone());
        assert!(mixed.contains_tag(Tag::Int) && mixed.contains_tag(Tag::Vector));
        assert_eq!(mixed.elem_ty(), vi.elem_ty());
    }

    #[test]
    fn element_disjointness_is_tags_only() {
        // vector<int> and vector<string> overlap (both vectors) — not disjoint, so
        // no false positive off an element mismatch.
        assert!(!Ty::vector_of(Ty::of(Tag::Int)).is_disjoint(&Ty::vector_of(Ty::of(Tag::Str))));
        // a vector and an int are disjoint (tags don't overlap).
        assert!(Ty::vector_of(Ty::of(Tag::Int)).is_disjoint(&Ty::of(Tag::Int)));
    }

    // ---- keyword-literal (singleton) types — ADR, keyword-only slice ----

    /// `(or :a :b)` as a `Ty` — the union of two keyword singletons.
    fn kw_union(names: &[&str]) -> Ty {
        names
            .iter()
            .map(|n| Ty::keyword_lit(value::intern(n)))
            .reduce(|a, b| a.union(b))
            .unwrap()
    }

    #[test]
    fn keyword_literal_renders_as_its_value() {
        assert_eq!(Ty::keyword_lit(value::intern("maximized")).to_string(), ":maximized");
        // a union keeps both (set-union is exact, not a widening); rendered sorted.
        assert_eq!(kw_union(&["a", "b"]).to_string(), ":a | :b");
        // mixed with another tag: the literals plus the open tag.
        assert_eq!(
            kw_union(&["maximized", "fullscreen"])
                .union(Ty::of(Tag::Nil))
                .to_string(),
            ":fullscreen | :maximized | nil"
        );
    }

    #[test]
    fn keyword_literal_union_is_exact_but_open_keyword_widens() {
        // {:a} ∪ {:b} = {:a, :b} — exact, both kept.
        let u = kw_union(&["a", "b"]);
        let mut want = BTreeSet::new();
        want.insert(value::intern("a"));
        want.insert(value::intern("b"));
        assert_eq!(u.as_lit(), Some(&want));
        // {:a} ∪ keyword(any) → any keyword (open side wins).
        let widened = Ty::keyword_lit(value::intern("a")).union(Ty::of(Tag::Keyword));
        assert!(widened.contains_tag(Tag::Keyword));
        assert_eq!(widened.as_lit(), None);
    }

    #[test]
    fn keyword_literal_subtyping() {
        let ab = kw_union(&["a", "b"]);
        // :a <: (:a | :b)
        assert!(Ty::keyword_lit(value::intern("a")).is_subtype(&ab));
        // (:a | :b) <: keyword(any)
        assert!(ab.is_subtype(&Ty::of(Tag::Keyword)));
        // :c ⊄ (:a | :b)
        assert!(!Ty::keyword_lit(value::intern("c")).is_subtype(&ab));
        // any keyword ⊄ a specific literal set
        assert!(!Ty::of(Tag::Keyword).is_subtype(&ab));
    }

    #[test]
    fn keyword_literal_disjointness_is_precise() {
        let ab = kw_union(&["a", "b"]);
        // :c is provably not one of (:a | :b) → disjoint → the checker can warn.
        assert!(Ty::keyword_lit(value::intern("c")).is_disjoint(&ab));
        // :a overlaps → not disjoint.
        assert!(!Ty::keyword_lit(value::intern("a")).is_disjoint(&ab));
        // any keyword could be :a → NOT provably disjoint (no false positive).
        assert!(!Ty::of(Tag::Keyword).is_disjoint(&ab));
        // a non-keyword is disjoint by tags as before.
        assert!(ab.is_disjoint(&Ty::of(Tag::Int)));
        // sharing another tag (nil) means not disjoint even if keywords differ.
        let c_or_nil = Ty::keyword_lit(value::intern("c")).union(Ty::of(Tag::Nil));
        let ab_or_nil = ab.clone().union(Ty::of(Tag::Nil));
        assert!(!c_or_nil.is_disjoint(&ab_or_nil));
    }

    #[test]
    fn keyword_literal_intersection() {
        // (:a | :b) ∩ (:b | :c) = {:b}
        let inter = kw_union(&["a", "b"]).intersect(kw_union(&["b", "c"]));
        let mut want = BTreeSet::new();
        want.insert(value::intern("b"));
        assert_eq!(inter.as_lit(), Some(&want));
        // (:a) ∩ (:b) = never (empty literal set clears the keyword tag).
        let empty = Ty::keyword_lit(value::intern("a")).intersect(Ty::keyword_lit(value::intern("b")));
        assert!(empty.is_never());
        // (:a | :b) ∩ keyword(any) = (:a | :b) (narrower wins).
        let narrowed = kw_union(&["a", "b"]).intersect(Ty::of(Tag::Keyword));
        assert_eq!(narrowed.as_lit(), kw_union(&["a", "b"]).as_lit());
    }

    #[test]
    fn of_value_makes_a_keyword_singleton() {
        let t = Ty::of_value(value::kw("maximized"));
        assert_eq!(t.to_string(), ":maximized");
        assert!(t.is_subtype(&Ty::of(Tag::Keyword)));
    }
}
