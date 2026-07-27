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
    Tag::Bytes,
    Tag::Decimal,
    Tag::Set,
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

/// The vector tag alone (not `pair` too) — the one tag a *positional* tuple
/// refinement applies to. Deliberately narrower than `SEQ_BITS`: a tuple is a
/// fixed-arity, per-position-typed shape, which only ever makes sense for a
/// `[ ]` vector (ADR-003 already keeps vectors and cons-list `pair`s
/// separate) — a `pair`-based list's length isn't part of its type the way a
/// vector literal's positions are.
const VECTOR_BIT: u32 = 1u32 << bit(Tag::Vector);

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
/// tree) goes superlinear (KI-13). Generous for real shapes — a record with its `:__id__`
/// plus a handful of fields is well under it — so only pathological structure hits it.
const MAX_TY_NODES: usize = 64;

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
    fields: Option<Arc<BTreeMap<Symbol, (Ty, bool)>>>,
    /// Refinement of the vector member (`vector`) to a **positional** shape —
    /// one type per index, when statically known from a `(tuple …)`
    /// annotation (ADR-128). `None` means "no declared positional shape".
    /// Mutually exclusive with `elem` in practice (a `Ty` is built by either
    /// `vector_of` or `tuple_of`), same independent-fields story as
    /// `map_kv`/`fields` above — see `docs/type-tuples.md`.
    tuple: Option<Arc<Vec<Ty>>>,
    /// Refinement of the keyword member (`keyword`) to a literal set — the exact
    /// keyword symbols admitted, e.g. `{:maximized, :fullboth}`. `None` means "any
    /// keyword". When `Some`, the `Keyword` bit is in `tags` and the set is
    /// non-empty; the keyword member is constrained to the set while every *other*
    /// tag in `tags` stays open (so `(or :a :b nil)` admits the two keywords *and*
    /// `nil`). Unlike the other refinements, union of two literal sets is *exact*
    /// (the set-union), not a widening — so `(or :a :b)` keeps both.
    lit: Option<Arc<BTreeSet<Symbol>>>,
    /// Refinement of the int member (`int`) to a literal set — the exact
    /// integers admitted, e.g. `{5, 6}` (ADR-117). Independent of `lit`
    /// (a different tag, `INT_BIT` not `KEYWORD_BIT`), so both can be `Some`
    /// at once (`(or :ok 5)`). Same semantics as `lit` throughout: union is
    /// exact, not a widening; every other tag stays open. `BigInt`-range
    /// literals aren't representable here — see `docs/type-int-literals.md`.
    lit_int: Option<Arc<BTreeSet<i64>>>,
    /// Refinement of the bool member (`bool`) to a literal set (ADR-120) —
    /// `{true}`, `{false}`, or (equivalent to unrefined) `{true, false}`.
    /// Independent tag/field, same semantics as `lit`/`lit_int` throughout.
    lit_bool: Option<Arc<BTreeSet<bool>>>,
    /// Refinement of the string member (`string`) to a literal set (ADR-120).
    /// Stores owned `String` content, not a heap `StrId` — two textually
    /// identical string literals can have different underlying heap handles,
    /// so comparing/ordering by content (not handle identity) is what makes
    /// set operations correct. Independent tag/field, same semantics as
    /// `lit`/`lit_int` throughout.
    lit_str: Option<Arc<BTreeSet<String>>>,
}

impl Ty {
    /// `⊥` — the empty set; the type of no value. A subtype of every type.
    pub const NEVER: Ty = Ty::flat(0);
    /// `⊤` — every tag; the type of any value. A supertype of every type.
    pub const ANY: Ty = Ty::flat(UNIVERSE);
    /// `int ∪ float ∪ decimal` — the named union the prelude's `number?` predicate
    /// implies. A `decimal` is a number (but not an integer).
    pub const NUMBER: Ty =
        Ty::flat((1u32 << bit(Tag::Int)) | (1u32 << bit(Tag::Float)) | (1u32 << bit(Tag::Decimal)));
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
            overload: None,
            fields: None,
            tuple: None,
            lit: None,
            lit_int: None,
            lit_bool: None,
            lit_str: None,
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
            arrow: Some(Arc::new(sig)),
            ..Ty::flat(FN_BITS)
        }
    }

    /// The function-arrow refinement, if this type carries one. The bridge the
    /// advisory checker reads to compare a callback against what a higher-order
    /// function expects.
    pub fn as_arrow(&self) -> Option<&Sig> {
        self.arrow.as_deref()
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
        self.overload.as_deref()
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
    pub fn record_of(fields: BTreeMap<Symbol, (Ty, bool)>) -> Ty {
        Ty {
            fields: Some(Arc::new(fields)),
            ..Ty::flat(MAP_BIT)
        }
        .bounded()
    }

    /// The record-shape refinement, if this map type carries one. The bridge
    /// the checker reads to flow `(get r :name)` to the field's exact type.
    pub fn record_fields(&self) -> Option<&BTreeMap<Symbol, (Ty, bool)>> {
        self.fields.as_deref()
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

    /// The tuple-shape refinement, if this vector type carries one. The
    /// bridge the checker reads to flow `(nth t i)`/`(first t)` to the exact
    /// per-position type.
    pub fn tuple_elems(&self) -> Option<&Vec<Ty>> {
        self.tuple.as_deref()
    }

    /// A keyword-literal (singleton) type — exactly the keyword `sym`. Unions of
    /// these build an enumerated keyword type, e.g. `(or :maximized :fullboth)`.
    pub fn keyword_lit(sym: Symbol) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(sym);
        Ty {
            lit: Some(Arc::new(set)),
            ..Ty::flat(KEYWORD_BIT)
        }
    }

    /// The keyword-literal refinement, if this type carries one (the exact keyword
    /// symbols admitted). `None` means "any keyword" (or no keyword member).
    pub fn as_lit(&self) -> Option<&BTreeSet<Symbol>> {
        self.lit.as_deref()
    }

    /// An int-literal (singleton) type — exactly the integer `n` (ADR-117).
    /// Unions of these build an enumerated int type, e.g. `(or 200 404 500)`.
    /// Independent of [`Ty::keyword_lit`] (a different tag), so the two
    /// compose freely — `(or :ok 5)` carries both refinements at once.
    pub fn int_lit(n: i64) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(n);
        Ty {
            lit_int: Some(Arc::new(set)),
            ..Ty::flat(INT_BIT)
        }
    }

    /// The int-literal refinement, if this type carries one (the exact
    /// integers admitted). `None` means "any int" (or no int member).
    pub fn as_lit_int(&self) -> Option<&BTreeSet<i64>> {
        self.lit_int.as_deref()
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
            lit_bool: Some(Arc::new(set)),
            ..Ty::flat(BOOL_BIT)
        }
    }

    /// The bool-literal refinement, if this type carries one. `None` means
    /// "any bool" (or no bool member).
    pub fn as_lit_bool(&self) -> Option<&BTreeSet<bool>> {
        self.lit_bool.as_deref()
    }

    /// A string-literal (singleton) type — exactly the string `s` (ADR-120).
    /// Takes `&str` rather than a `Value`/`Heap` pair — the caller reads the
    /// content out of its `Value::Str` heap handle first (`heap.string(id)`),
    /// so `Ty` itself stays heap-independent like every other constructor.
    pub fn str_lit(s: &str) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(s.to_string());
        Ty {
            lit_str: Some(Arc::new(set)),
            ..Ty::flat(STR_BIT)
        }
    }

    /// The string-literal refinement, if this type carries one. `None` means
    /// "any string" (or no string member).
    pub fn as_lit_str(&self) -> Option<&BTreeSet<String>> {
        self.lit_str.as_deref()
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
        self.elem.as_deref().cloned().or_else(|| {
            self.tuple
                .as_ref()
                .map(|elems| elems.iter().cloned().fold(Ty::NEVER, |acc, t| acc.union(t)))
        })
    }

    /// The type of a concrete value — the bridge from a runtime value to its type.
    /// A keyword becomes its **literal singleton** (`:foo`, not the whole `keyword`
    /// tag), so a literal in code is checked against an enumerated keyword sig.
    /// Ints deliberately stay flat here (unlike keywords) — see
    /// `docs/type-int-literals.md`'s "Deferred" section: making every int
    /// literal in code a singleton cascades into every misuse-warning message
    /// that happens to mention a literal int (7 existing tests broke on exact
    /// wording, e.g. "got int" → "got 5"), a materially bigger and riskier
    /// change than this slice's scope (declared-sig literal sets).
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
            for (t, _) in f.values() {
                n += t.node_count(lim - n);
                if n >= lim {
                    return n;
                }
            }
        }
        if let Some(ts) = &self.tuple {
            for t in ts.iter() {
                n += t.node_count(lim - n);
                if n >= lim {
                    return n;
                }
            }
        }
        n
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
        {
            return self;
        }
        if self.node_count(MAX_TY_NODES + 1) > MAX_TY_NODES {
            Ty {
                arrow: None,
                overload: None,
                elem: None,
                map_kv: None,
                fields: None,
                tuple: None,
                ..self
            }
        } else {
            self
        }
    }

    pub fn union(self, other: Ty) -> Ty {
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
        let fields = merge_union(
            self.tags & MAP_BIT != 0,
            &self.fields,
            other.tags & MAP_BIT != 0,
            &other.fields,
        );
        let tuple = merge_union(
            self.tags & VECTOR_BIT != 0,
            &self.tuple,
            other.tags & VECTOR_BIT != 0,
            &other.tuple,
        );
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
        let lit_bool = merge_union_lit_set(
            BOOL_BIT,
            self.tags,
            &self.lit_bool,
            other.tags,
            &other.lit_bool,
        );
        let lit_str = merge_union_lit_set(
            STR_BIT,
            self.tags,
            &self.lit_str,
            other.tags,
            &other.lit_str,
        );
        Ty {
            tags,
            arrow,
            overload,
            elem,
            map_kv,
            fields,
            tuple,
            lit,
            lit_int,
            lit_bool,
            lit_str,
        }
        // Bound the result's size so a fixpoint that unions nested branch results (a
        // recursive value-builder) can't grow the type without limit — every subsequent
        // `union`/`is_subtype`/`==` input then stays within the cap (KI-13).
        .bounded()
    }

    /// `self ∩ other` — values in both. When the relevant bit survives and one
    /// side is unrefined ("any"), the other side's refinement is the narrower —
    /// keep it; two distinct known refinements can't be one → widen. (Used by
    /// guard narrowing `T ∩ tested_by(pred)`, where `tested_by` is flat, so a
    /// refined `T` keeps its refinement through the narrow.)
    pub fn intersect(self, other: Ty) -> Ty {
        let mut tags = self.tags & other.tags;
        let (arrow, overload) = if tags & FN_BITS != 0 {
            intersect_arrows(&self, &other)
        } else {
            (None, None)
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
        let fields = if tags & MAP_BIT != 0 {
            merge_intersect(&self.fields, &other.fields)
        } else {
            None
        };
        let tuple = if tags & VECTOR_BIT != 0 {
            merge_intersect(&self.tuple, &other.tuple)
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
        let lit_bool = if tags & BOOL_BIT != 0 {
            let (s, keep) = intersect_lit_set(&self.lit_bool, &other.lit_bool);
            if !keep {
                tags &= !BOOL_BIT;
            }
            s
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
        Ty {
            tags,
            arrow,
            overload,
            elem,
            map_kv,
            fields,
            tuple,
            lit,
            lit_int,
            lit_bool,
            lit_str,
        }
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
    pub fn negate(self) -> Ty {
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
                    if !self_candidates.iter().any(|s| s.is_subtype(req)) {
                        return false;
                    }
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
                let self_elem = self.elem.clone().or_else(|| {
                    self.tuple.as_ref().map(|elems| {
                        Arc::new(elems.iter().cloned().fold(Ty::NEVER, |acc, t| acc.union(t)))
                    })
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
                        // `map<K,V>` when a keyword key fits K and every field value type
                        // fits V. (Uses the conservative "any keyword" for the key rather
                        // than the exact literal set — enough for the common `map<keyword,
                        // any>`, and never a false accept.)
                        Some(fields) => {
                            if !Ty::of(Tag::Keyword).is_subtype(&b.0) {
                                return false;
                            }
                            for (vty, _opt) in fields.values() {
                                if !vty.is_subtype(&b.1) {
                                    return false;
                                }
                            }
                        }
                        None => return false, // self = "any map" ⊄ a specific map<K,V>
                    },
                }
            }
            if let Some(b) = &other.fields {
                match &self.fields {
                    Some(a) => {
                        if !record_fields_is_subtype(a, b) {
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
        true
    }

    /// Do `self` and `other` share no values? (`self ∩ other = ⊥`.) Tag overlap
    /// decides it, with two *precise* exceptions: when the only shared tag is
    /// `keyword` (or, independently, `int`) and both sides pin disjoint literal
    /// sets, no value of that tag satisfies both. This only ever *adds*
    /// genuinely-disjoint cases (a literal set is an exact enumeration, not an
    /// approximation), so it can't raise a false warning — advisory-soundness
    /// holds.
    pub fn is_disjoint(&self, other: &Ty) -> bool {
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
                return a.iter().zip(b.iter()).any(|(x, y)| x.is_disjoint(y));
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
                return a.iter().any(|(name, (aty, areq))| {
                    b.get(name)
                        .is_some_and(|(bty, breq)| (*areq || *breq) && aty.is_disjoint(bty))
                });
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
fn record_fields_is_subtype(
    self_fields: &BTreeMap<Symbol, (Ty, bool)>,
    other_fields: &BTreeMap<Symbol, (Ty, bool)>,
) -> bool {
    for (name, (other_ty, other_required)) in other_fields {
        match self_fields.get(name) {
            None => return false,
            Some((self_ty, self_required)) => {
                if *other_required && !*self_required {
                    return false;
                }
                if !self_ty.is_subtype(other_ty) {
                    return false;
                }
            }
        }
    }
    true
}

/// `self <: other` for two tuple shapes: exact arity match (unlike a record's
/// open width-subtyping, a tuple's arity *is* its shape — a 2-tuple isn't a
/// subtype of a 3-tuple, and vice versa), then covariant per position — sound
/// because Brood vectors are immutable, same reasoning as element-covariant
/// sequences.
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
    a: &Option<Arc<BTreeSet<T>>>,
    b_tags: u32,
    b: &Option<Arc<BTreeSet<T>>>,
) -> Option<Arc<BTreeSet<T>>> {
    let open = |tags: u32, set: &Option<Arc<BTreeSet<T>>>| tags & tag != 0 && set.is_none();
    if open(a_tags, a) || open(b_tags, b) {
        return None;
    }
    if a.is_none() && b.is_none() {
        return None;
    }
    let mut set = BTreeSet::new();
    if let Some(a) = a {
        set.extend(a.iter().cloned());
    }
    if let Some(b) = b {
        set.extend(b.iter().cloned());
    }
    if set.is_empty() {
        None
    } else {
        Some(Arc::new(set))
    }
}

/// The surviving literal set for an **intersection** of one tag's literal member
/// (the tag bit already known to survive): the narrower of the two — two sets
/// intersect exactly; an *open* side (no set) intersects to the other side's set.
/// The returned `bool` is `false` when the intersection is empty, so no value of
/// the tag qualifies and the caller clears the tag bit.
fn intersect_lit_set<T: Ord + Clone>(
    a: &Option<Arc<BTreeSet<T>>>,
    b: &Option<Arc<BTreeSet<T>>>,
) -> (Option<Arc<BTreeSet<T>>>, bool) {
    match (a, b) {
        (Some(a), Some(b)) => {
            let s: BTreeSet<T> = a.intersection(b).cloned().collect();
            if s.is_empty() {
                (None, false)
            } else {
                (Some(Arc::new(s)), true)
            }
        }
        (Some(a), None) => (Some(a.clone()), true),
        (None, Some(b)) => (Some(b.clone()), true),
        (None, None) => (None, true),
    }
}

/// Is `self`'s literal member for one tag a subtype of `other`'s? `self_has_tag`
/// is whether `self` carries the tag at all (only then is there anything to
/// check). An unrefined `other` admits everything; a refined `other` requires a
/// refined `self` subset — an open `self` ("any") is *not* a subset of a literal
/// set.
fn lit_is_subtype<T: Ord>(
    self_has_tag: bool,
    a: &Option<Arc<BTreeSet<T>>>,
    b: &Option<Arc<BTreeSet<T>>>,
) -> bool {
    if !self_has_tag {
        return true;
    }
    match b {
        None => true,
        Some(b) => match a {
            Some(a) => a.is_subset(b),
            None => false,
        },
    }
}

/// Whether two literal sets decide **disjointness** for a tag that is the sole
/// shared tag: `Some(_)` when both sides pin a set (an exact enumeration), else
/// `None` (the caller falls through to its default). Only ever adds a
/// genuinely-disjoint verdict — advisory-soundness holds.
fn lit_disjoint<T: Ord>(
    shared_is_tag: bool,
    a: &Option<Arc<BTreeSet<T>>>,
    b: &Option<Arc<BTreeSet<T>>>,
) -> Option<bool> {
    if shared_is_tag {
        if let (Some(a), Some(b)) = (a, b) {
            return Some(a.is_disjoint(b));
        }
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
            // Some inhabited materialisation fits — i.e. `bound` is not *provably
            // disjoint* from `expected`. Uses [`Ty::is_disjoint`], not
            // `intersect().is_never()`: the two agree on flat tags, but only
            // `is_disjoint` also sees the refinement-level conflicts (record
            // fields, tuple shapes, literal sets), so a dynamic value with a
            // refined type that provably can't fit is caught here too.
            !self.bound.is_disjoint(&expected)
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
mod tests;
