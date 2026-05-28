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

use std::fmt;

use crate::core::value::{self, Tag, Value};

/// Every tag, in bit order — for iterating a `Ty`'s members (printing, etc.) and
/// the source of [`TAG_COUNT`]. **Must list every [`Tag`] variant in discriminant
/// order**; the compiler can't enumerate variants, so `tag_universe_is_consistent`
/// (below) is what guards completeness, ordering, and the universe size.
const ALL_TAGS: [Tag; 15] = [
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
];

/// The number of tag atoms — derived from [`ALL_TAGS`], not hand-counted.
const TAG_COUNT: u32 = ALL_TAGS.len() as u32;
/// `Ty` is a `u16`, so at most 16 atoms fit. The `UNIVERSE` mask
/// `(1u16 << TAG_COUNT) - 1` would otherwise fail const-eval with a cryptic
/// shift-overflow message when someone added the 17th atom — this surfaces
/// the cap with a clear message right where the lattice width is set. Widen
/// `Ty(u16)` to `Ty(u32)` (and this assert) to lift the cap.
const _: () = assert!(
    TAG_COUNT <= 16,
    "Ty is u16-wide; widen the type to add more than 16 atoms",
);
/// All bits set for the atoms — the universe `⊤`. Follows [`TAG_COUNT`].
const UNIVERSE: u16 = (1u16 << TAG_COUNT) - 1;

/// The bit position of `tag` in a [`Ty`]'s bitset — its `#[repr(u8)]`
/// discriminant. No hand-maintained mapping (so no collisions possible); the
/// declaration order of [`Tag`] is the bit order.
const fn bit(tag: Tag) -> u32 {
    tag as u8 as u32
}

/// A set-theoretic type: a set of runtime [`Tag`]s. `Copy` and cheap (one `u16`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ty(u16);

impl Ty {
    /// `⊥` — the empty set; the type of no value. A subtype of every type.
    pub const NEVER: Ty = Ty(0);
    /// `⊤` — every tag; the type of any value. A supertype of every type.
    pub const ANY: Ty = Ty(UNIVERSE);
    /// `int ∪ float` — the named union the prelude's `number?` predicate implies.
    pub const NUMBER: Ty = Ty::of(Tag::Int).union(Ty::of(Tag::Float));
    /// `nil ∪ pair` — the named union the prelude's `list?` predicate implies.
    pub const LIST: Ty = Ty::of(Tag::Nil).union(Ty::of(Tag::Pair));

    /// The singleton type containing exactly the values with this tag.
    pub const fn of(tag: Tag) -> Ty {
        Ty(1u16 << bit(tag))
    }

    /// The type of a concrete value — the bridge from a runtime value to its type.
    pub fn of_value(v: Value) -> Ty {
        Ty::of(value::tag(v))
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
            "symbol?" => Ty::of(Tag::Sym),
            "keyword?" => Ty::of(Tag::Keyword),
            "string?" => Ty::of(Tag::Str),
            "pair?" => Ty::of(Tag::Pair),
            "vector?" => Ty::of(Tag::Vector),
            "map?" => Ty::of(Tag::Map),
            "ref?" => Ty::of(Tag::Ref),
            "pid?" => Ty::of(Tag::Pid),
            // `fn?` holds for both Brood closures and Rust builtins.
            "fn?" => Ty::of(Tag::Fn).union(Ty::of(Tag::Native)),
            "number?" => Ty::NUMBER,
            "list?" => Ty::LIST,
            _ => return None,
        })
    }

    /// `self ∪ other` — values in either.
    pub const fn union(self, other: Ty) -> Ty {
        Ty(self.0 | other.0)
    }

    /// `self ∩ other` — values in both.
    pub const fn intersect(self, other: Ty) -> Ty {
        Ty(self.0 & other.0)
    }

    /// `¬self` — every value *not* in `self` (complemented within the universe).
    pub const fn negate(self) -> Ty {
        Ty(!self.0 & UNIVERSE)
    }

    /// `self \ other` — values in `self` but not `other`.
    pub const fn difference(self, other: Ty) -> Ty {
        self.intersect(other.negate())
    }

    /// `self ⊆ other` — semantic subtyping: is every value of `self` a value of
    /// `other`?
    pub const fn is_subtype(self, other: Ty) -> bool {
        self.0 & other.0 == self.0
    }

    /// Do `self` and `other` share no values? (`self ∩ other = ⊥`.)
    pub const fn is_disjoint(self, other: Ty) -> bool {
        self.intersect(other).is_never()
    }

    /// Does this type admit a value with `tag`?
    pub const fn contains_tag(self, tag: Tag) -> bool {
        self.0 & (1u16 << bit(tag)) != 0
    }

    /// Is this the empty type `⊥` (no value inhabits it)?
    pub const fn is_never(self) -> bool {
        self.0 == 0
    }

    /// Is this the universe `⊤` (every value inhabits it)?
    pub const fn is_any(self) -> bool {
        self.0 == UNIVERSE
    }
}

impl fmt::Display for Ty {
    /// A readable rendering for diagnostics: the named lattice points where they
    /// apply (`never`, `any`, `number`, `list`), a single tag by its `type-of`
    /// name, otherwise the members joined with ` | ` (e.g. `int | string`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Ty::NEVER => return f.write_str("never"),
            Ty::ANY => return f.write_str("any"),
            Ty::NUMBER => return f.write_str("number"),
            Ty::LIST => return f.write_str("list"),
            _ => {}
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Debug)]
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
        self.params.get(i).copied().or(self.rest)
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
    pub const fn is_dynamic(self) -> bool {
        self.dynamic
    }

    /// **Consistent subtyping** into a static expectation — derived from set
    /// inclusion, the relation a checker uses for "can a value of this gradual
    /// type be used where `expected` is wanted?". Static: `bound ⊆ expected`.
    /// Dynamic: some inhabited materialisation fits, `bound ∩ expected ≠ ⊥`.
    pub fn consistent_with(self, expected: Ty) -> bool {
        if self.dynamic {
            !self.bound.intersect(expected).is_never()
        } else {
            self.bound.is_subtype(expected)
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
        assert_eq!(Ty::NUMBER, Ty::of(Tag::Int).union(Ty::of(Tag::Float)));
        assert_eq!(Ty::LIST, Ty::of(Tag::Nil).union(Ty::of(Tag::Pair)));
        assert!(Ty::of(Tag::Int).contains_tag(Tag::Int));
        assert!(!Ty::of(Tag::Int).contains_tag(Tag::Float));
    }

    #[test]
    fn subtyping_is_set_inclusion() {
        assert!(Ty::of(Tag::Int).is_subtype(Ty::NUMBER)); // int ⊆ number
        assert!(Ty::NUMBER.is_subtype(Ty::ANY)); // number ⊆ any
        assert!(!Ty::NUMBER.is_subtype(Ty::of(Tag::Int))); // number ⊄ int
                                                           // ⊥ is a subtype of everything; everything is a subtype of ⊤.
        assert!(Ty::NEVER.is_subtype(Ty::of(Tag::Str)));
        assert!(Ty::of(Tag::Str).is_subtype(Ty::ANY));
        assert!(Ty::of(Tag::Int).is_subtype(Ty::of(Tag::Int))); // reflexive
    }

    #[test]
    fn intersection_and_disjointness() {
        assert_eq!(Ty::NUMBER.intersect(Ty::of(Tag::Int)), Ty::of(Tag::Int));
        assert_eq!(Ty::NUMBER.intersect(Ty::of(Tag::Str)), Ty::NEVER);
        assert!(Ty::NUMBER.is_disjoint(Ty::LIST));
        assert!(!Ty::NUMBER.is_disjoint(Ty::of(Tag::Float)));
    }

    #[test]
    fn negation_and_difference() {
        assert_eq!(Ty::NEVER.negate(), Ty::ANY);
        assert_eq!(Ty::ANY.negate(), Ty::NEVER);
        let not_nil = Ty::of(Tag::Nil).negate();
        assert!(!not_nil.contains_tag(Tag::Nil));
        assert!(not_nil.contains_tag(Tag::Int));
        // number \ int = float
        assert_eq!(Ty::NUMBER.difference(Ty::of(Tag::Int)), Ty::of(Tag::Float));
    }

    #[test]
    fn of_value_bridges_runtime_values() {
        // These Value variants are heap-free, so no Heap is needed.
        assert_eq!(Ty::of_value(Value::Int(1)), Ty::of(Tag::Int));
        assert_eq!(Ty::of_value(Value::Nil), Ty::of(Tag::Nil));
        assert_eq!(Ty::of_value(Value::Bool(true)), Ty::of(Tag::Bool));
        assert!(Ty::of_value(Value::Int(1)).is_subtype(Ty::NUMBER));
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
            assert!(Ty::of(*tag).is_subtype(Ty::ANY));
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
                d.consistent_with(t),
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
        let s = sample_tys();
        for &a in &s {
            assert_eq!(a.union(Ty::NEVER), a, "∪⊥ identity");
            assert_eq!(a.intersect(Ty::ANY), a, "∩⊤ identity");
            assert_eq!(a.union(a), a, "∪ idempotent");
            assert_eq!(a.intersect(a), a, "∩ idempotent");
            assert_eq!(a.union(a.negate()), Ty::ANY, "complement ∪");
            assert_eq!(a.intersect(a.negate()), Ty::NEVER, "complement ∩");
            assert_eq!(a.negate().negate(), a, "double negation");
            for &b in &s {
                assert_eq!(a.union(b), b.union(a), "∪ commutes");
                assert_eq!(a.intersect(b), b.intersect(a), "∩ commutes");
                // subtyping IS set inclusion: a ⊆ b ⟺ a ∩ b = a
                assert_eq!(a.is_subtype(b), a.intersect(b) == a, "subtype ⟺ inclusion");
                // disjoint IS empty intersection
                assert_eq!(a.is_disjoint(b), a.intersect(b).is_never(), "disjoint ⟺ ∅");
                // De Morgan
                assert_eq!(
                    a.union(b).negate(),
                    a.negate().intersect(b.negate()),
                    "De Morgan"
                );
            }
        }
    }

    #[test]
    fn subtyping_is_reflexive_and_transitive() {
        let s = sample_tys();
        for &a in &s {
            assert!(a.is_subtype(a));
            for &b in &s {
                for &c in &s {
                    if a.is_subtype(b) && b.is_subtype(c) {
                        assert!(a.is_subtype(c), "subtype transitivity");
                    }
                }
            }
        }
    }
}
