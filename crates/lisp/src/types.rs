//! The type lattice — step 1 of the set-theoretic type direction (ADR-023,
//! inspired by Elixir's set-theoretic + gradual type system).
//!
//! A [`Ty`] **is a set of values**, represented as a bitset over the runtime
//! [`Tag`]s (the value-set atoms — see [`crate::value::Tag`]). On this model the
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

use std::fmt;

use crate::value::{self, Tag, Value};

/// The number of tag atoms (must match the [`Tag`] variants).
const TAG_COUNT: u32 = 12;
/// All bits set for the 12 atoms — the universe `⊤`.
const UNIVERSE: u16 = (1u16 << TAG_COUNT) - 1;

/// The bit position of each tag in a [`Ty`]'s bitset. Exhaustive over [`Tag`], so
/// adding a tag is a compile error here until it's given a bit.
const fn bit(tag: Tag) -> u32 {
    match tag {
        Tag::Nil => 0,
        Tag::Bool => 1,
        Tag::Int => 2,
        Tag::Float => 3,
        Tag::Sym => 4,
        Tag::Keyword => 5,
        Tag::Str => 6,
        Tag::Pair => 7,
        Tag::Vector => 8,
        Tag::Fn => 9,
        Tag::Macro => 10,
        Tag::Native => 11,
    }
}

/// Every tag, low bit first — for iterating a `Ty`'s members (printing, etc.).
const ALL_TAGS: [Tag; TAG_COUNT as usize] = [
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
];

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

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
}
