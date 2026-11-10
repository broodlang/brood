//! The set operations on ONE term of a [`Ty`] — union, intersection and complement of a
//! single term, slot by slot — and the size bound and depth widening that keep every type
//! finite. The operations over a whole union of terms are built on these
//! (`types/union_of_terms.rs`).

use super::*;

impl Ty {
    /// `self ∪ other` — values in either. A refinement survives only where it's
    /// unambiguous: if just one side contributes the relevant members (functions
    /// for `arrow`, sequences for `elem`), that side's refinement carries; if both
    /// do, it survives only when they agree (the union of two distinct
    /// arrows/element-types isn't a single one → widen to "any"). Widening is
    /// sound: a union is a supertype anyway.
    /// Bounded node count of this `Ty`'s refinement tree, walked as a tree and stopping
    /// once it reaches `lim` (so a shared `Arc` DAG that is exponential as a tree can't
    /// make the count itself blow up). Used only by [`bounded`](Ty::bounded).
    pub(super) fn node_count(&self, lim: usize) -> usize {
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
            // An arrow whose parameters would widen is DROPPED, not widened: parameters
            // are contravariant, so a wider domain is a NARROWER arrow, and widening it
            // shrank the type. Only an arm that survives intact keeps its (widened) result.
            out.arrow = self
                .arrow
                .as_ref()
                .and_then(|s| widen_sig(s, depth - 1))
                .map(Arc::new);
            if let Some(arms) = &self.overload {
                // Dropping an arm of an intersection widens it — the sound direction.
                let kept: Vec<Sig> = arms
                    .iter()
                    .filter_map(|s| widen_sig(s, depth - 1))
                    .collect();
                out.overload = None;
                match kept.len() {
                    0 => {}
                    1 => out.arrow = kept.into_iter().next().map(Arc::new),
                    _ => out.overload = Some(Arc::new(kept)),
                }
            }
        }
        // The alternatives and the subtraction sit at THIS level.
        out.alts = self
            .alts
            .as_ref()
            .map(|a| Arc::new(a.iter().map(|t| t.widened_below(depth)).collect()));
        // A subtracted term is the other way round: `P ∖ N` grows as `N` SHRINKS, so
        // widening `N` would narrow the whole — `(not vector<int>)` came out `(not vector)`,
        // which excludes vectors the original admits, and the Pass 2.9 fixpoint read that
        // as its parameter's type. A subtraction that is already within the depth stays;
        // one that would have to widen is dropped, which widens the whole (sound).
        let kept: Vec<Ty> = self
            .neg
            .iter()
            .flat_map(|n| n.iter())
            .filter(|t| t.widened_below(depth) == **t)
            .cloned()
            .collect();
        out.neg = (!kept.is_empty()).then(|| Arc::new(kept));
        out
    }
    /// If this type's refinement tree exceeds [`MAX_TY_NODES`] nodes, widen it to the flat
    /// tag set (dropping the structural refinements; the bounded literal sets are kept).
    /// Sound — widening over-approximates, never manufacturing a false positive — and it
    /// bounds every `Ty` to a fixed size so `union`/`==`/`Hash`/`is_subtype` stay linear.
    pub(super) fn bounded(self) -> Ty {
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

    pub(super) fn union_term(self, other: Ty) -> Ty {
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
            self.tags & SHAPE_BITS != 0,
            &self.fields,
            other.tags & SHAPE_BITS != 0,
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
    pub(super) fn intersect_term(self, other: Ty) -> Ty {
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
        // Key and value types meet EXACTLY: a map in both has every key in `K1 ∩ K2` and
        // every value in `V1 ∩ V2`, and any such map is in both. The generic merge widened
        // two different `map<K, V>`s to a bare `map` — not even below either operand.
        let map_kv = if tags & MAP_BIT != 0 {
            match (&self.map_kv, &other.map_kv) {
                (Some(a), Some(b)) if a != b => Some(Arc::new((
                    a.0.clone().intersect(b.0.clone()),
                    a.1.clone().intersect(b.1.clone()),
                ))),
                (a, b) => merge_intersect(a, b),
            }
        } else {
            None
        };
        // Record shapes intersect **exactly** (see `intersect_records`) rather than
        // widening to `None` the way the generic refinement merge does — this is the
        // narrowing a type guard performs, and dropping it would lose the fact the
        // guard just established. A shape pair no value satisfies clears the `map` bit.
        let fields = if tags & SHAPE_BITS != 0 {
            match (&self.fields, &other.fields) {
                (Some(a), Some(b)) => match intersect_records(a, b) {
                    Some(shape) => Some(Arc::new(shape)),
                    None => {
                        tags &= !SHAPE_BITS;
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
        .settle_subtraction()
    }

    /// A term's subtractions with the irrelevant ones gone — a subtracted term the positive
    /// part shares nothing with removes nothing, and keeping it made `"abc" ∩ (not (tuple
    /// int))` a different `Ty` from `"abc"` — and an EMPTY term as [`Ty::NEVER`] itself, so
    /// that `list<int> ∩ list<string>` (no pair has a head of both) compares equal to the
    /// empty type it denotes rather than to an uninhabited shape that only `is_never` sees
    /// through.
    pub(super) fn settle_subtraction(mut self) -> Ty {
        if let Some(negs) = &self.neg {
            let positive = self.positive();
            let kept: Vec<Ty> = negs
                .iter()
                .filter(|n| !positive.is_disjoint(n))
                .cloned()
                .collect();
            self.neg = (!kept.is_empty()).then(|| Arc::new(kept));
        }
        if self.term_is_never() {
            return Ty::NEVER;
        }
        self
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
    pub(super) fn negate_term(self) -> Ty {
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
        if self.map_kv.is_some() {
            tags |= self.tags & MAP_BIT;
        }
        if self.fields.is_some() {
            tags |= self.tags & SHAPE_BITS;
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
}

/// [`Ty::widened_below`] over a signature: the result widens (covariant); the parameters
/// must come through UNCHANGED, or the arrow has no sound widening at this depth and the
/// answer is `None` — a parameter is contravariant, so widening it narrows the arrow.
fn widen_sig(sig: &Sig, depth: usize) -> Option<Sig> {
    let unchanged = |t: &Ty| t.widened_below(depth) == *t;
    if !sig.params.iter().all(unchanged)
        || !sig.optional.iter().all(unchanged)
        || !sig.rest.iter().all(unchanged)
    {
        return None;
    }
    let mut out = sig.clone();
    out.ret = sig.ret.widened_below(depth);
    // A guard names exactly the type its truthy result proves; widened, it would prove
    // more than the predicate tests.
    if out.guard.is_some() && out.ret != sig.ret {
        out.guard = None;
    }
    Some(out)
}
