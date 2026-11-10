//! The slot-merging rules behind a union or an intersection of terms: when two terms merge
//! into one without loss, how each refinement slot (arrow, element, positional shape,
//! literal set, interval, length) combines, the widening collapse a fixpoint uses, and the
//! canonical forms an interval or literal slot is kept in.

use super::*;

/// Would the widening union of these two terms lose nothing — is their union
/// representable as one term?
///
/// It is, unless some refinement slot is *contested*: both sides contribute the tags
/// that slot refines, and they carry different refinements there. That is exactly the
/// case [`merge_union`] answers with `None` (widen to "any of that tag"), and exactly
/// the case a second term now exists to hold instead. One rule per slot, matching the
/// slots and tag masks `union_term` uses.
pub(super) fn merge_is_exact(a: &Ty, b: &Ty) -> bool {
    // A self-reference merges with nothing but another self-reference: merged into a
    // term it would either drop the term (unsound) or read as `any` (imprecise) — kept
    // as its own alternative it stays exactly what it is.
    if a.rec_ref != b.rec_ref {
        return false;
    }
    // A closed recursive type merges with nothing either: merged into a term, the binder
    // would come to range over the OTHER term too — `json ∪ list` read as μX.(json-body[X]
    // ∪ list), whose `vector<X>` then admits vectors of lists — and `A ∪ B` came out a
    // different (wider) type than `B ∪ A`, which kept the binder as its own alternative.
    if a.mu || b.mu {
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
    let (ra, rb) = (a.tags & SHAPE_BITS != 0, b.tags & SHAPE_BITS != 0);
    let (va, vb) = (a.tags & VECTOR_BIT != 0, b.tags & VECTOR_BIT != 0);
    let (pa, pb) = (a.tags & PAIR_BIT != 0, b.tags & PAIR_BIT != 0);
    !contested(fa, &a.arrow, fb, &b.arrow)
        && !contested(fa, &a.overload, fb, &b.overload)
        && (!contested(sa, &a.elem, sb, &b.elem) || elem_union_exact(a, b).is_some())
        && !contested(ma, &a.map_kv, mb, &b.map_kv)
        && !contested(ra, &a.fields, rb, &b.fields)
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
pub(super) fn positional_union(
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
pub(super) fn positional_union_is_exact(
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
pub(super) fn elem_union_exact(a: &Ty, b: &Ty) -> Option<Option<Arc<Ty>>> {
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
        if a.elem == b.elem {
            return None; // identical: the plain rule keeps it
        }
        // The same elements, stated once as a shape and once as an element type
        // (`(list S) ∪ list<S>`): the plain rule reads two different slots and widens to
        // no element type at all — which, once a lone shape stopped carrying a redundant
        // `elem` beside it, turned a path accumulator `nil | list<S>` into bare `list`.
        return Some((ea != Ty::ANY).then(|| Arc::new(ea)));
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
/// Whether a record field's type is a `defrecord` identity — the `:__id__` slot of a
/// nominal record holds keyword literals only (one per member).
pub(super) fn is_record_identity(ty: &Ty) -> bool {
    ty.tags == KEYWORD_BIT && ty.lit.as_ref().is_some_and(|l| l.members().is_some())
}

pub(super) fn merge_union<T: PartialEq>(
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
pub(super) fn merge_intersect<T: PartialEq>(
    a: &Option<Arc<T>>,
    b: &Option<Arc<T>>,
) -> Option<Arc<T>> {
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
pub(super) fn merge_union_lit_set<T: Ord + Clone>(
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
pub(super) fn collapse_same_tags(terms: Vec<Ty>) -> Vec<Ty> {
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
pub(super) fn positional_hull(
    a: &Option<Arc<Vec<Ty>>>,
    b: &Option<Arc<Vec<Ty>>>,
) -> Option<Arc<Vec<Ty>>> {
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
pub(super) fn tagged_apart(a: &Ty, b: &Ty) -> bool {
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
pub(super) fn merge_ranges(a_has: bool, a: Range, b_has: bool, b: Range) -> Option<Range> {
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
pub(super) fn canon_int_range(
    lit_int: &Option<Arc<LitSet<i64>>>,
    range: Option<Range>,
) -> Option<Range> {
    if matches!(lit_int.as_deref(), Some(LitSet::In(_))) {
        return None;
    }
    range.filter(|r| !r.is_all())
}

/// The canonical length beside the tags: none without a countable member, none for
/// `ALL`, and none for `[1, ∞)` on a term whose only countable member is `pair` (which
/// is at least 1 by being a pair).
pub(super) fn canon_len(tags: u32, len: Option<Range>) -> Option<Range> {
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
pub(super) fn filter_lit_int(
    lit_int: Option<Arc<LitSet<i64>>>,
    r: Range,
) -> Option<Arc<LitSet<i64>>> {
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
pub(super) fn intersect_lit_set<T: Ord + Clone>(
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

pub(super) fn lit_is_subtype<T: Ord>(
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
pub(super) fn lit_disjoint<T: Ord>(
    shared_is_tag: bool,
    a: &Option<Arc<LitSet<T>>>,
    b: &Option<Arc<LitSet<T>>>,
) -> Option<bool> {
    if shared_is_tag && (a.is_some() || b.is_some()) {
        return Some(lit_sets_disjoint(a.as_deref(), b.as_deref()));
    }
    None
}
