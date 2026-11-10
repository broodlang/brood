//! Recursive types (ADR-349): the μ binder and its self-reference, unrolling and
//! substitution, folding a fixpoint's ascent into a recursive type, and the coinductive
//! assumptions and budgets that make every relation and meet over one terminate.

use super::*;

impl Ty {
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

    /// Drop the binder when nothing refers to it any more (a widening or a merge took
    /// the references with it), so a non-recursive type never claims to be one.
    pub(super) fn normalise_mu(mut self) -> Ty {
        if self.mu && !self.has_rec_ref() {
            self.mu = false;
        }
        self
    }

    /// Is `any` — every tag, no refinement, no subtraction — as a single term?
    pub(super) fn is_top_term(&self) -> bool {
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
        if let Some(e) = &self.elem
            && pred(e)
        {
            return true;
        }
        if let Some(kv) = &self.map_kv
            && (pred(&kv.0) || pred(&kv.1))
        {
            return true;
        }
        if let Some(f) = &self.fields
            && (f.fields.values().any(|(t, _)| pred(t)) || pred(&f.rest))
        {
            return true;
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
        if let Some(negs) = &self.neg
            && negs.iter().any(&mut *pred)
        {
            return true;
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
        self.body_terms()
            .into_iter()
            .map(|term| {
                if term.mu {
                    term
                } else {
                    term.map_term_slots(f)
                }
            })
            .fold(Ty::NEVER, Ty::union)
    }

    /// This single term with every directly nested Ty replaced by `f` of it — no
    /// re-union, so the term's own shape is kept as it is.
    fn map_term_slots(&self, f: &mut dyn FnMut(&Ty) -> Ty) -> Ty {
        let mut out = self.clone();
        out.elem = self.elem.as_ref().map(|e| Arc::new(f(e)));
        out.map_kv = self
            .map_kv
            .as_ref()
            .map(|kv| Arc::new((f(&kv.0), f(&kv.1))));
        out.fields = self.fields.as_ref().map(|shape| {
            Arc::new(RecordShape {
                fields: shape
                    .fields
                    .iter()
                    .map(|(k, (t, req))| (*k, (f(t), *req)))
                    .collect(),
                rest: f(&shape.rest),
            })
        });
        out.tuple = self
            .tuple
            .as_ref()
            .map(|ts| Arc::new(ts.iter().map(&mut *f).collect()));
        out.list_shape = self
            .list_shape
            .as_ref()
            .map(|ts| Arc::new(ts.iter().map(&mut *f).collect()));
        let map_sig = |s: &Sig, f: &mut dyn FnMut(&Ty) -> Ty| Sig {
            params: s.params.iter().map(&mut *f).collect(),
            rest: s.rest.as_ref().map(&mut *f),
            ret: f(&s.ret),
            ..s.clone()
        };
        out.arrow = self.arrow.as_ref().map(|s| Arc::new(map_sig(s, f)));
        out.overload = self
            .overload
            .as_ref()
            .map(|o| Arc::new(o.iter().map(|s| map_sig(s, f)).collect()));
        out.neg = self
            .neg
            .as_ref()
            .map(|ns| Arc::new(ns.iter().map(&mut *f).collect()));
        out
    }

    /// One level of the recursion: this type with each self-reference replaced by the
    /// type itself. Not a binder afterwards — the copies inside are. A non-recursive
    /// type is its own unrolling.
    pub fn unroll(&self) -> Ty {
        self.unroll_with(true)
    }

    /// [`Ty::unroll`] for a RELATION ([`with_rec_assumption`]) to compare: the same set,
    /// but the unions the substitution creates are written down as they stand instead of
    /// re-canonicalised. Canonicalising a union asks `⊆` of its terms, and those terms
    /// hold the recursive type — so unrolling for one relation ran relations that unrolled
    /// the same type again, each level with a pair the coinductive assumptions did not
    /// hold yet. That nesting is exponential in the type's alternatives: a reply type of
    /// three error records whose `data` is the type itself never finished one `⊆ map`. A
    /// relation needs only the set; a slot with no self-reference is left as it is.
    fn unroll_for_relation(&self) -> Ty {
        self.unroll_with(false)
    }

    fn unroll_with(&self, canonical: bool) -> Ty {
        if !self.mu {
            return self.clone();
        }
        let whole = self.clone();
        let body = Ty {
            mu: false,
            ..self.clone()
        };
        body.subst_rec(&whole, canonical)
    }

    /// Every self-reference in this type (outside a nested binder) replaced by `with`;
    /// the unions it rebuilds canonical, or as written (see [`Ty::unroll_for_relation`]).
    fn subst_rec(&self, with: &Ty, canonical: bool) -> Ty {
        if self.mu {
            return self.clone(); // a nested binder's references are its own
        }
        if !canonical && !self.has_rec_ref() {
            return self.clone();
        }
        // A self-reference TERM is replaced by the whole of `with` (a union), so the
        // terms are rebuilt as a union.
        let terms: Vec<Ty> = self
            .body_terms()
            .into_iter()
            .map(|term| {
                if term.rec_ref {
                    with.clone()
                } else if term.mu {
                    term
                } else {
                    term.map_term_slots(&mut |t| t.subst_rec(with, canonical))
                }
            })
            .collect();
        if canonical {
            terms.into_iter().fold(Ty::NEVER, Ty::union)
        } else {
            Ty::from_terms_as_written(terms)
        }
    }

    /// A union of `terms` with nothing absorbed or merged — only `never` and exact
    /// duplicates dropped: the same SET as [`Ty::from_terms`], in a form only a relation
    /// may see. Falls back to the canonical union where a bare term list cannot hold the
    /// operand: a binder over a union, or binders alone.
    fn from_terms_as_written(terms: Vec<Ty>) -> Ty {
        let mut kept: Vec<Ty> = Vec::with_capacity(terms.len());
        for term in terms {
            if term.alts.is_some() && term.mu {
                kept.push(term);
                return Ty::from_terms(kept);
            }
            for inner in term.terms_vec() {
                if !inner.is_never() && !kept.contains(&inner) {
                    kept.push(inner);
                }
            }
        }
        if kept.is_empty() {
            return Ty::NEVER;
        }
        // As in `from_terms`: a binder at the head of a union would range over all of it.
        if kept.len() > 1 && kept[0].mu {
            match kept.iter().position(|t| !t.mu) {
                Some(plain) => kept.swap(0, plain),
                None => return Ty::from_terms(kept),
            }
        }
        let mut head = kept.remove(0);
        if !kept.is_empty() {
            head.alts = Some(Arc::new(kept));
        }
        head
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
}

thread_local! {
    /// The pairs of recursive types under comparison right now — the coinductive
    /// assumptions of [`with_rec_assumption`].
    static REC_ASSUMPTIONS: std::cell::RefCell<Vec<(Ty, Ty)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Unrollings spent by the outermost [`with_rec_assumption`] in progress (reset when
    /// one starts) — the budget [`MAX_REC_UNROLLS`] bounds, and what a test reads.
    static REC_UNROLLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// How often [`MAX_REC_UNROLLS`] cut a relation short on this thread.
    static REC_BUDGET_EXHAUSTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many unrollings one recursive relation may spend. Far past any shape inference
/// builds — a pair repeats within a few levels — so it never decides a real question; it
/// exists so that no recursive type, however its alternatives multiply, can keep the
/// checker from finishing.
const MAX_REC_UNROLLS: usize = 2_000;

/// The unrollings the last outermost recursive relation on this thread spent.
#[cfg(test)]
pub(crate) fn rec_unrolls_spent() -> usize {
    REC_UNROLLS.with(std::cell::Cell::get)
}

/// How often the unrolling budget has cut a relation short on this thread.
#[cfg(test)]
pub(crate) fn rec_budget_exhaustions() -> usize {
    REC_BUDGET_EXHAUSTED.with(std::cell::Cell::get)
}

/// Compare `a` and `b` — at least one recursive — through their unrollings, under the
/// assumption that the pair itself satisfies the relation: a re-encounter of the same
/// pair inside answers `assumed` (true for an inclusion, false for a disjointness —
/// the answer that is sound when the question rests on itself). Regular types have
/// finitely many distinct pairs of subterms, so the descent terminates.
///
/// Bounded twice over: the assumptions cut every cycle, and [`MAX_REC_UNROLLS`] caps the
/// unrollings one outermost question may spend, past which the answer is `false` — "not
/// a subtype" / "not disjoint", the sound reading of a question left undecided.
pub(super) fn with_rec_assumption(
    a: &Ty,
    b: &Ty,
    assumed: bool,
    relate: impl Fn(&Ty, &Ty) -> bool,
) -> bool {
    let (seen, outermost) = REC_ASSUMPTIONS.with(|s| {
        let s = s.borrow();
        (s.iter().any(|(x, y)| x == a && y == b), s.is_empty())
    });
    if seen {
        return assumed;
    }
    let spent = REC_UNROLLS.with(|count| {
        let spent = if outermost { 1 } else { count.get() + 1 };
        count.set(spent);
        spent
    });
    if spent > MAX_REC_UNROLLS {
        #[cfg(test)]
        REC_BUDGET_EXHAUSTED.with(|count| count.set(count.get() + 1));
        return false;
    }
    REC_ASSUMPTIONS.with(|s| s.borrow_mut().push((a.clone(), b.clone())));
    let out = crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        relate(&a.unroll_for_relation(), &b.unroll_for_relation())
    });
    REC_ASSUMPTIONS.with(|s| {
        s.borrow_mut().pop();
    });
    out
}

thread_local! {
    /// The pairs of types whose meet [`meet_recursive`] is computing right now.
    static REC_MEETS: std::cell::RefCell<Vec<(Ty, Ty)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// How many recursive meets may nest before [`meet_recursive`] stops descending. Far past
/// any shape inference builds (a pair repeats within a level or two); it exists so a pair
/// that never repeats exactly — the unrollings growing apart — still terminates.
const MAX_REC_MEET_DEPTH: usize = 32;

/// `a ∩ b` where at least one side is recursive: the meet of their unrollings.
///
/// Two recursive types meet THEMSELVES again one level down — `json ∩ json2` unrolls to
/// `… | vector<json> | …` against `… | vector<json2> | …`, whose elements are the same
/// pair — so the descent has to stop somewhere, or it overflows the stack (it did: any
/// `deftype`d recursive type met with another, a guard narrowing a `json` parameter, aborted
/// the checker with exit 134). Inclusion has the coinductive assumption for this; a meet
/// cannot assume its own answer, so the re-encountered pair is answered by a SOUND
/// over-approximation — one of the two operands, each a supertype of the true meet. The
/// result is exact down to where the pair repeats and wider below it, which can only
/// suppress a warning. (The usual case never gets here: `json ∩ json` and `json ∩
/// vector<json>` are decided exactly by the inclusion shortcut in [`Ty::intersect`].)
///
/// The operand chosen is the one with the smaller rendering, so `a ∩ b` and `b ∩ a` agree.
pub(super) fn meet_recursive(a: Ty, b: Ty) -> Ty {
    let (repeated, depth) = REC_MEETS.with(|s| {
        let s = s.borrow();
        let repeated = s
            .iter()
            .any(|(x, y)| (x == &a && y == &b) || (x == &b && y == &a));
        (repeated, s.len())
    });
    if repeated || depth >= MAX_REC_MEET_DEPTH {
        return if a.to_string() <= b.to_string() { a } else { b };
    }
    REC_MEETS.with(|s| s.borrow_mut().push((a.clone(), b.clone())));
    let out = crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || a.unroll().intersect(b.unroll()));
    REC_MEETS.with(|s| {
        s.borrow_mut().pop();
    });
    out
}
