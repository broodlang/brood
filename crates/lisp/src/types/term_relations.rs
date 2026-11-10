//! The free helpers the term relations of [`Ty`] are built from: record-shape subtyping,
//! disjointness and intersection, positional-shape intersection, the covering rules that
//! decide whether one term sits inside a union of others (products, records, intersections
//! of arrows), and the canonical form of an arrow intersection.

use super::*;

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
pub(super) fn record_is_subtype(a: &RecordShape, b: &RecordShape) -> bool {
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
pub(super) fn records_are_disjoint(a: &RecordShape, b: &RecordShape) -> bool {
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
pub(super) fn intersect_records(a: &RecordShape, b: &RecordShape) -> Option<RecordShape> {
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
pub(super) fn intersect_tuples(a: &[Ty], b: &[Ty]) -> Option<Vec<Ty>> {
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
pub(super) fn union_negs(
    a: &Option<Arc<Vec<Ty>>>,
    b: &Option<Arc<Vec<Ty>>>,
) -> Option<Arc<Vec<Ty>>> {
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
pub(super) fn arrows_cover(candidates: &[Sig], req: &Sig) -> bool {
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
pub(super) fn tuple_covered_by(a: &[Ty], candidates: &[Vec<Ty>]) -> bool {
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

/// Is a record shape covered by several shapes together (C9)?
///
/// A record is a product over the keys any of the shapes declares — a key none declares
/// on one side reads as that shape's `rest` (`nil` closed, `any` open), which is exactly
/// `RecordShape::field_ty`, the one reading every relation uses — so the declared keys
/// are the positions of [`tuple_covered_by`]. The **undeclared remainder is not a
/// position**: it stands for infinitely many independent keys, and a map holding `1`
/// under one and `"a"` under another is a `{…: int|string}` that neither `{…: int}` nor
/// `{…: string}` contains. So the rest behaves like a vector's elements — it must fit a
/// single surviving candidate's rest — which is the base case here, where the tuple rule
/// has "some candidate survives".
pub(super) fn record_covered_by(a: &RecordShape, candidates: &[&RecordShape]) -> bool {
    let keys: BTreeSet<Symbol> = a
        .fields
        .keys()
        .chain(candidates.iter().flat_map(|c| c.fields.keys()))
        .copied()
        .collect();
    let row =
        |shape: &RecordShape| -> Vec<Ty> { keys.iter().map(|k| shape.field_ty(*k)).collect() };
    let a_row = row(a);
    let rows: Vec<(Vec<Ty>, &Ty)> = candidates.iter().map(|c| (row(c), &c.rest)).collect();
    fn go(a: &[Ty], a_rest: &Ty, candidates: &[(Vec<Ty>, &Ty)]) -> bool {
        if a.is_empty() {
            return candidates.iter().any(|(_, rest)| a_rest.is_subtype(rest));
        }
        if candidates.len() > 8 {
            return false; // conservative, as `tuple_covered_by`
        }
        let n = candidates.len();
        for mask in 0..(1u32 << n) {
            let mut first = Ty::NEVER;
            let mut rest: Vec<(Vec<Ty>, &Ty)> = Vec::new();
            for (j, (cand, cand_rest)) in candidates.iter().enumerate() {
                if mask & (1 << j) != 0 {
                    first = first.union(cand[0].clone());
                } else {
                    rest.push((cand[1..].to_vec(), cand_rest));
                }
            }
            if a[0].is_subtype(&first) {
                continue;
            }
            if go(&a[1..], a_rest, &rest) {
                continue;
            }
            return false;
        }
        true
    }
    go(&a_row, &a.rest, &rows)
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
pub(super) fn term_covered(a: &Ty, others: &[Ty]) -> bool {
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
pub(super) fn term_is_subtype_of_union(a: &Ty, others: &[Ty]) -> bool {
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
    // Rust 1.98, and this crate builds on 1.97 — the two cannot both be satisfied by a bit
    // trick. `ALL_TAGS` is short, this is not a hot path, and it is the idiom the
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
        if tag_bit == VECTOR_BIT
            && let Some(elems) = part.tuple_elems()
        {
            let candidates: Vec<Vec<Ty>> = others
                .iter()
                .filter_map(|b| member_positions(b, VECTOR_BIT, elems.len()))
                // A different arity shares no value with this one.
                .filter(|c| c.len() == elems.len())
                .collect();
            if tuple_covered_by(elems, &candidates) {
                continue;
            }
        }
        // A list shape is a product too.
        if tag_bit == PAIR_BIT
            && let Some(elems) = part.list_shape_elems()
        {
            let candidates: Vec<Vec<Ty>> = others
                .iter()
                .filter_map(|b| member_positions(b, PAIR_BIT, elems.len()))
                .filter(|c| c.len() == elems.len())
                .collect();
            if tuple_covered_by(elems, &candidates) {
                continue;
            }
        }
        // And a record shape (C9): its declared keys are the positions of a
        // product, so `{a: int|string}` is covered by `{a: int} | {a: string}` the way a
        // 1-tuple is. The undeclared remainder is not a position — see `record_covered_by`.
        if (tag_bit == MAP_BIT || tag_bit == RECORD_BIT)
            && let Some(shape) = part.fields.as_deref()
        {
            // A candidate whose map member ALSO carries `map<K, V>` is the intersection of
            // the two, and the covering rule reads shapes alone: counted as its shape it
            // over-accepted — `{a: int, b: string}` read as covered by `(and (record
            // &open :a int) (map keyword int))`, so a union absorbed that record into a
            // term whose values cannot be strings, and a read of `:b` lost its string.
            // Left out, it can only fail to help cover (incomplete, never unsound).
            let candidates: Vec<&RecordShape> = others
                .iter()
                .filter(|b| b.tags & tag_bit != 0 && b.map_kv.is_none())
                .filter_map(|b| b.fields.as_deref())
                .collect();
            if record_covered_by(shape, &candidates) {
                continue;
            }
        }
        return false;
    }
    true
}

/// The positions one term holds for one positional member — its tuple (`VECTOR_BIT`) or
/// list shape (`PAIR_BIT`) — as a covering candidate: each position met with the term's
/// element type, which constrains the same member, and nothing when the term's length slot
/// excludes the arity. Reading the shape alone over-accepted: a candidate `(tuple int)`
/// carrying `elem: string` beside it holds no vector at all, yet covered `(tuple int)`.
/// Not restricted to a term that is nothing but that member — the shape refines the member
/// whatever else the term admits, so a mixed candidate covers its own vectors too.
///
/// A term with NO shape for the member still holds the vectors (lists) of `arity`
/// elements its element type admits — `vector<int>`'s 1-vectors are `(tuple int)` — so it
/// is a candidate too, with every position its element type. Without that, `(tuple (or
/// int string))` read as outside `vector<int> | vector<string>`, which holds every such
/// 1-vector between its two halves.
pub(super) fn member_positions(term: &Ty, member: u32, arity: usize) -> Option<Vec<Ty>> {
    if term.tags & member == 0 || term.neg.is_some() || term.rec_ref || term.mu {
        return None;
    }
    if let Some(len) = term.len
        && !Range::subset(Range::point(arity as i64), len)
    {
        return None;
    }
    let shape = if member == VECTOR_BIT {
        term.tuple.as_deref()
    } else {
        term.list_shape.as_deref()
    };
    let elem = term.elem.as_deref().cloned().unwrap_or(Ty::ANY);
    match shape {
        Some(shape) if shape.len() == arity => Some(
            shape
                .iter()
                .map(|t| t.clone().intersect(elem.clone()))
                .collect(),
        ),
        Some(_) => None,
        None => Some(vec![elem; arity]),
    }
}

pub(super) fn tuple_is_subtype(self_elems: &[Ty], other_elems: &[Ty]) -> bool {
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
pub(super) fn candidate_sigs(ty: &Ty) -> Vec<Sig> {
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
pub(super) fn intersect_arrows(a: &Ty, b: &Ty) -> (Option<Arc<Sig>>, Option<Arc<Vec<Sig>>>) {
    let sa = candidate_sigs(a);
    let sb = candidate_sigs(b);
    if sa.is_empty() {
        return (b.arrow.clone(), b.overload.clone());
    }
    if sb.is_empty() {
        return (a.arrow.clone(), a.overload.clone());
    }
    let mut combined = sa;
    combined.extend(sb);
    let combined = canonical_arms(combined);
    if combined.len() == 1 {
        (
            Some(Arc::new(combined.into_iter().next().expect("len == 1"))),
            None,
        )
    } else {
        (None, Some(Arc::new(combined)))
    }
}

/// Would merging these two terms' intervals lose nothing? A merged term carries ONE int
/// interval and ONE length (over every countable member), the hull of the two sides'. That
/// is the union exactly when the two intervals overlap or touch — and, for a length, when
/// both sides count the same members (`string[7] ∪ vector[2]` hulls to a `[2..7]` that
/// admits a two-character string).
pub(super) fn ranges_merge_exactly(a: &Ty, b: &Ty) -> bool {
    fn contiguous(x: Range, y: Range) -> bool {
        let grown = Range::new(y.lo.map(|lo| lo - 1), y.hi.map(|hi| hi + 1));
        Range::meet(x, grown).is_some()
    }
    let both_pinned = matches!(a.lit_int.as_deref(), Some(LitSet::In(_)))
        && matches!(b.lit_int.as_deref(), Some(LitSet::In(_)));
    if a.tags & b.tags & INT_BIT != 0
        && !both_pinned
        && !contiguous(a.int_range_eff(), b.int_range_eff())
    {
        return false;
    }
    if a.tags & COUNT_BITS != 0 && b.tags & COUNT_BITS != 0 {
        let (la, lb) = (a.len_eff(), b.len_eff());
        if la != lb && (a.tags & COUNT_BITS != b.tags & COUNT_BITS || !contiguous(la, lb)) {
            return false;
        }
    }
    true
}

/// The arms of an arrow intersection in canonical form. An arm another arm already
/// implies adds nothing — `(int -> int) and (int -> any)` is `(int -> int)` — and the
/// arms are a SET: kept in arrival order and unabsorbed, `A ∩ B` and `B ∩ A` were
/// different `Ty`s, and `a ⊆ b` did not give `a ∩ b == a`. Sorted by rendering; of two
/// arms that imply each other (one set, two spellings) the first in that order stays.
pub(super) fn canonical_arms(mut arms: Vec<Sig>) -> Vec<Sig> {
    arms.sort_by_cached_key(Sig::to_string);
    arms.dedup();
    let mut kept: Vec<Sig> = Vec::with_capacity(arms.len());
    for (i, sig) in arms.iter().enumerate() {
        let implied = arms
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && other.is_subtype(sig) && (!sig.is_subtype(other) || j < i));
        if !implied {
            kept.push(sig.clone());
        }
    }
    kept
}
