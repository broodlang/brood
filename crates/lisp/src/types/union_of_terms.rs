//! The public set operations and relations of [`Ty`] over a union of terms (ADR-262):
//! building a type from its terms, `union`, `intersect`, `negate`, `difference`,
//! `is_subtype`, `is_consistent_subtype` and `is_disjoint`, with the per-term subtyping and
//! disjointness they quantify over.

use super::*;

impl Ty {
    // ---- the union of terms (ADR-262) ----
    //
    // Everything above operates on ONE term. These five are the public set
    // operations, and they quantify over a type's terms — which is what makes a
    // union of two structured types (`(or (tuple int) (tuple string))`) survive
    // being written down instead of widening to bare `vector`.

    /// This type's terms: itself when single (the common case), else the head term
    /// followed by its alternatives. Every term is alts-free.
    pub(super) fn terms_vec(&self) -> Vec<Ty> {
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
    pub(super) fn body_terms(&self) -> Vec<Ty> {
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
    pub(super) fn from_terms(terms: Vec<Ty>) -> Ty {
        Ty::from_terms_with(terms, false)
    }

    /// [`Ty::from_terms`], optionally refusing the one merge a union makes on purpose that
    /// is not exact: two intervals (an int's, a length's) that neither overlap nor touch
    /// merge to their HULL (ADR-350 — what keeps an index's type one term). An
    /// intersection distributed over terms must not do that: `vector<int> ∩ (vector<string>
    /// | vector<int>[2])` is `vector<never> | vector<int>[2]`, and hulled to one term it
    /// read `vector<int>` — not below its operands.
    pub(super) fn from_terms_with(mut terms: Vec<Ty>, exact_ranges: bool) -> Ty {
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
                if merge_is_exact(existing, &t)
                    && (!exact_ranges || ranges_merge_exactly(existing, &t))
                {
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
        //
        // And a term may be covered by SEVERAL others together — `(tuple (or int string))`
        // by `vector<int> | vector<string>` — which no pairwise test sees; kept, `a ⊆ b` did
        // not give `a ∪ b == b`. Asked only when more than one other term remains.
        let mut i = 0;
        while i < merged.len() {
            let absorbed = merged
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && contains(other, &merged[i]))
                || (merged.len() > 2 && {
                    let others: Vec<Ty> = merged
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, t)| t.clone())
                        .collect();
                    term_covered(&merged[i], &others)
                });
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
        // The head of a union doubles as its container, and a container flagged `mu` is a
        // binder over the WHOLE union. A closed recursive alternative must therefore not
        // sit at the head beside others, or its binder would come to range over them.
        if merged.len() > 1
            && merged[0].mu
            && let Some(plain) = merged.iter().position(|t| !t.mu)
        {
            merged.swap(0, plain);
        }
        let mut head = merged.remove(0);
        if !merged.is_empty() {
            head.alts = Some(Arc::new(merged));
        }
        head
    }

    /// `self ∪ other` — every value in either.
    pub fn union(self, other: Ty) -> Ty {
        // A closed recursive operand stays its own alternative (`merge_is_exact` refuses
        // it, `from_terms` keeps it off the head), so `int ∪ μX.(nil | vector<X>)` is
        // exact and the same in either order. The flag below matters only for an operand
        // whose references are still OPEN (a body under construction).
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
        // An operand inside the other IS the meet. Exact, and it is what makes the result
        // canonical where the slot-wise meet below is not: that one keeps a subtraction
        // the other side already excludes (`"abc" ∩ (not (tuple int))`), lists an arrow
        // the other already implies, and carries both of two spellings of one set — so
        // `a ⊆ b` did not give `a ∩ b == a`, and every memo keyed on the result missed.
        // It is also the whole answer for the common recursive case, `json ∩ json`.
        if self.is_subtype(&other) {
            return self;
        }
        if other.is_subtype(&self) {
            return other;
        }
        // A μ is met through its unrolling, so the references inside the result are the
        // recursive type itself rather than a dangling placeholder — under a guard that
        // ties the knot, since two recursive types meet their own pair again inside.
        if self.mu || other.mu {
            return meet_recursive(self, other);
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
        Ty::from_terms_with(out, true)
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
        if self.mu {
            return self.unknowns_as_never().is_subtype(other);
        }
        // Term by term, because one filling is not like the others: a `pair` has a head,
        // so `pair<never>` is EMPTY, and filling a bare `pair`'s unknown elements with
        // `never` turned a value positively known to be a list into the empty type — a
        // consistent subtype of everything. `(sig f (pair -> int))` then passed its
        // parameter to `%string-length` silently, in both modes, where `(list any)` warned.
        // A pair's materialisation must keep a pair, with SOME element: that fits exactly
        // when `other` admits such a pair at all, which is the overlap reading.
        self.terms_vec().into_iter().all(|term| {
            let filled = term.unknowns_as_never();
            if filled.is_never() && !term.is_never() {
                return !term.is_disjoint(other);
            }
            filled.is_subtype(other)
        })
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
            } else if let Some(rest) = ty.without_unknown_keyed() {
                // The unknown keyed value (`map | record`, ADR-542) is a nested unknown
                // too: which kind it is was never known, so it fills like `?`.
                fill(&rest)
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
            if term.tags & SHAPE_BITS != 0 {
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
    pub(super) fn project_tag(&self, tag_bit: u32) -> Ty {
        fn keep<T>(applies: bool, slot: &Option<Arc<T>>) -> Option<Arc<T>> {
            if applies { slot.clone() } else { None }
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
            fields: keep(tag_bit & SHAPE_BITS != 0, &self.fields),
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
            // The term's length is the HULL over every countable member it has, so on one
            // member it is only a bound — and a tuple's arity is the exact one. The
            // normalisation below lets the shape win (and drops a member whose arity the
            // bound excludes); copied through as-is, `len_eff` preferred the hull, and the
            // vector half of `(tuple 1 2) | "abcdefg"` read as `vector[2..7]`, outside a
            // `(len (vector int) 2 2)` it was in.
            len: if tag_bit & COUNT_BITS != 0 {
                self.len
            } else {
                None
            },
            alts: None,
        }
        .normalise_len()
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
    pub(super) fn is_subtype_term(&self, other: &Ty) -> bool {
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
        if self.tags & SEQ_BITS != 0
            && let Some(b) = &other.elem
        {
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
        if self.tags & VECTOR_BIT != 0
            && let Some(b) = &other.tuple
        {
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
        if self.tags & PAIR_BIT != 0
            && let Some(b) = &other.list_shape
        {
            match &self.list_shape {
                Some(a) => {
                    if !tuple_is_subtype(a, b) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if self.tags & MAP_BIT != 0
            && let Some(b) = &other.map_kv
        {
            {
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
                        // (A `defrecord` value's shape is tagged `record`, never `map`, so
                        // it never reaches here — ADR-542.)
                        Some(shape) => {
                            let entries = || shape.fields.iter();
                            if shape.is_open() {
                                if !Ty::of(Tag::Keyword).is_subtype(&b.0) {
                                    return false;
                                }
                            } else if !entries()
                                .all(|(name, _)| Ty::keyword_lit(*name).is_subtype(&b.0))
                            {
                                return false;
                            }
                            for (_, (vty, _opt)) in entries() {
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
        }
        // A shape — a map's or a record's (ADR-542) — must have `other`'s.
        if self.tags & SHAPE_BITS != 0
            && let Some(b) = &other.fields
        {
            match &self.fields {
                Some(a) => {
                    if !record_is_subtype(a, b) {
                        return false;
                    }
                }
                None => return false, // self doesn't provably have `other`'s shape
            }
        }
        // Each literal member: every value `self` admits for the tag must be one
        // `other` admits (an unrefined `other` admits all; an open `self` is not a
        // subset of a specific literal set). One rule per independent tag/field.
        // A small bounded interval IS a literal set — `(int 1 2)` is `1 | 2` — so it is
        // enumerated when `other` pins a set; otherwise an interval was never inside one.
        let self_lit_int = self.lit_int.clone().or_else(|| {
            other.lit_int.as_ref()?;
            self.enumerated_int_range()
        });
        if !lit_is_subtype(self.tags & KEYWORD_BIT != 0, &self.lit, &other.lit)
            || !lit_is_subtype(self.tags & INT_BIT != 0, &self_lit_int, &other.lit_int)
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
    pub(super) fn is_disjoint_term(&self, other: &Ty) -> bool {
        if self.rec_ref || other.rec_ref {
            return false; // an unknown set is never PROVABLY disjoint
        }
        let shared = self.tags & other.tags;
        if shared == 0 {
            return true;
        }
        // When the sole shared tag is a literal kind and both sides pin disjoint
        // sets, no value of that tag satisfies both. One rule per independent tag.
        //
        // Only a PROVEN disjointness returns: overlapping literal sets may still be wholly
        // subtracted (`¬"x" ∩ ¬string[3]` shares nothing with `string[3]`), and returning the
        // negative answer here skipped the subtraction rule below that knows it.
        if lit_disjoint(shared == KEYWORD_BIT, &self.lit, &other.lit) == Some(true)
            || lit_disjoint(shared == INT_BIT, &self.lit_int, &other.lit_int) == Some(true)
            || lit_disjoint(shared == BOOL_BIT, &self.lit_bool, &other.lit_bool) == Some(true)
            || lit_disjoint(shared == STR_BIT, &self.lit_str, &other.lit_str) == Some(true)
        {
            return true;
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
        // …and two list shapes, the same way (a list has exactly one length). A side with
        // an ELEMENT type and no shape holds, at the other side's arity, exactly the
        // shape of that many elements — `vector<int>` against `(tuple string)` meets at
        // `(tuple int) ∩ (tuple string)`, empty, which `intersect` already knew.
        for member in [VECTOR_BIT, PAIR_BIT] {
            if shared != member {
                continue;
            }
            let shape_of = |t: &Ty| {
                if member == VECTOR_BIT {
                    t.tuple.as_deref().map(Vec::len)
                } else {
                    t.list_shape.as_deref().map(Vec::len)
                }
            };
            if let (Some(a), Some(b)) = (shape_of(self), shape_of(other))
                && a != b
            {
                return true;
            }
            let Some(arity) = shape_of(self).or_else(|| shape_of(other)) else {
                continue;
            };
            // A side whose length slot excludes the shape's arity holds no such member.
            if [self, other].iter().any(|t| {
                t.len
                    .is_some_and(|len| !Range::subset(Range::point(arity as i64), len))
            }) {
                return true;
            }
            // Only a PROVEN disjointness returns here. Returning the negative answer too
            // would skip the subtraction check below, which is the one that knows
            // `(tuple int|string) ∖ (tuple int)` shares nothing with `(tuple int)`.
            if let (Some(a), Some(b)) = (
                member_positions(&self.positive(), member, arity),
                member_positions(&other.positive(), member, arity),
            ) && a.iter().zip(b.iter()).any(|(x, y)| x.is_disjoint(y))
            {
                return true;
            }
        }
        // Two NON-EMPTY lists are provably disjoint when their element types are: a
        // `list<T>` is the `pair` tag alone — the empty list is `nil`, a separate tag —
        // so every value of `list<int>` has a first element, and no first element is
        // both an `int` and a `string`. The same fact `intersect` states by making
        // `list<int> ∩ list<string>` the uninhabited `list<never>`; the two must agree
        // or the argument check (`!is_disjoint`) and the lattice contradict each other.
        if shared == (1u32 << bit(Tag::Pair))
            && let (Some(a), Some(b)) = (self.elem_ty(), other.elem_ty())
            && a.is_disjoint(&b)
        {
            return true;
        }
        // Two vectors (sets) whose element types are disjoint share only the EMPTY one —
        // so they share nothing when either side's length excludes 0. (`intersect` reads
        // `vector<string> ∩ vector<int>[2]` as `vector<never>[2]`, which is empty.)
        let set_bit = 1u32 << bit(Tag::Set);
        if shared & !(VECTOR_BIT | set_bit) == 0
            && let (Some(a), Some(b)) = (self.elem.as_deref(), other.elem.as_deref())
        {
            let zero = Range::point(0);
            if a.is_disjoint(b)
                && (!Range::subset(zero, self.len_eff()) || !Range::subset(zero, other.len_eff()))
            {
                return true;
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
        if (shared == MAP_BIT || shared == RECORD_BIT)
            && let (Some(a), Some(b)) = (&self.fields, &other.fields)
            && records_are_disjoint(a, b)
        {
            return true;
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
}
