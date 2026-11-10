//! The literal-set refinement [`LitSet`] — one tag's values stated positively (exactly
//! these) or negatively (anything but these) — with its canonical form and the union,
//! intersection, inclusion and disjointness of two sets over the same tag.

use super::*;

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
    pub(super) fn members(&self) -> Option<&BTreeSet<T>> {
        match self {
            LitSet::In(set) => Some(set),
            LitSet::Out(_) => None,
        }
    }

    /// The excluded values, or `None` for a positive set. For rendering.
    pub(super) fn excluded(&self) -> Option<&BTreeSet<T>> {
        match self {
            LitSet::Out(set) => Some(set),
            LitSet::In(_) => None,
        }
    }

    /// Is this set empty — i.e. does the tag admit no value at all? Only a positive
    /// empty set is; a negative one excludes finitely many from an infinite domain.
    pub(super) fn is_empty(&self) -> bool {
        matches!(self, LitSet::In(set) if set.is_empty())
    }
}

impl<T: Ord + Clone> LitSet<T> {
    /// This set's complement *within its tag*. `In ↔ Out`, exactly.
    pub(super) fn complement(&self) -> LitSet<T> {
        match self {
            LitSet::In(set) => LitSet::Out(set.clone()),
            LitSet::Out(set) => LitSet::In(set.clone()),
        }
    }
}

/// A literal slot in canonical form. Two spellings of the same set must not survive as
/// different `Ty`s: `Ty` derives its equality and hash from the slots, so `false | true`
/// comparing unequal to `bool` is not cosmetic — it makes `bool <: (or false true)` come
/// out **false** for two identical sets, which is a spurious warning waiting to happen,
/// and it breaks the fixpoint loops that iterate until a type stops changing.
///
/// Two ways a slot can say "every value of the tag", which is what `None` means:
/// excluding nothing, and — for **bool** alone, the one finite domain — listing both.
pub(super) fn canon_lit<T: Ord>(slot: Option<Arc<LitSet<T>>>) -> Option<Arc<LitSet<T>>> {
    match slot.as_deref() {
        Some(LitSet::Out(excluded)) if excluded.is_empty() => None,
        _ => slot,
    }
}

/// [`canon_lit`] plus bool's finite-domain rule: `{true, false}` is every bool.
pub(super) fn canon_lit_bool(slot: Option<Arc<LitSet<bool>>>) -> Option<Arc<LitSet<bool>>> {
    let slot = canon_lit(slot);
    match slot.as_deref() {
        Some(LitSet::In(set)) if set.len() == 2 => None,
        _ => slot,
    }
}

/// Union of two sets over the same tag. `None` on either side means "every value of the
/// tag", which absorbs.
pub(super) fn lit_union<T: Ord + Clone>(
    a: Option<&LitSet<T>>,
    b: Option<&LitSet<T>>,
) -> Option<LitSet<T>> {
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
pub(super) fn lit_intersect<T: Ord + Clone>(
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
pub(super) fn lit_subset<T: Ord>(a: Option<&LitSet<T>>, b: Option<&LitSet<T>>) -> bool {
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
pub(super) fn lit_sets_disjoint<T: Ord>(a: Option<&LitSet<T>>, b: Option<&LitSet<T>>) -> bool {
    match (a, b) {
        (None, _) | (_, None) => false,
        (Some(LitSet::In(x)), Some(LitSet::In(y))) => x.is_disjoint(y),
        (Some(LitSet::In(x)), Some(LitSet::Out(y)))
        | (Some(LitSet::Out(y)), Some(LitSet::In(x))) => x.is_subset(y),
        // Two infinite complements always overlap.
        (Some(LitSet::Out(_)), Some(LitSet::Out(_))) => false,
    }
}
