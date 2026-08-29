//! **Arrow subtyping, checked against what an arrow actually denotes.**
//!
//! ADR-292 made the relation more permissive: an intersection of arrows can satisfy a
//! requirement that no single conjunct satisfies. Every such change risks the one error this
//! lattice must never make — claiming `A <: B` when some value of `A` lies outside `B`. The
//! property laws (transitivity, the union bounds, disjointness against intersection) cannot
//! catch that on their own: they check the relation against *itself*, so a rule that is
//! uniformly too permissive can satisfy all of them.
//!
//! So this checks it against the semantics instead. Over a three-value universe an arrow is a
//! finite, enumerable set of functions — `(A → B)` is exactly those `f` with `f(v) ∈ B` for
//! every `v ∈ A` — and an intersection of arrows is the intersection of those sets.
//! Containment is then plain subset containment, computed by brute force, with no reference
//! to the checker's rules at all.
//!
//! **The soundness direction is valid despite the small universe.** Every arrow built here
//! has its domain inside `{int, bool, string}`, so a function's membership in either side
//! depends only on its behaviour there; a witness found on three values therefore extends to
//! a real function (send everything else to `int`) that witnesses the same failure in the
//! full 23-tag lattice. A finite model cannot prove the rule for all types, but it can refute
//! it, and that is what this is for.
//!
//! Three values give 27 functions, 8 types, 56 arrows and — with intersections of two —
//! 1596 candidate types, hence ~2.5M ordered pairs. Runs in a few seconds.

use brood::core::value::Tag;
use brood::types::{Sig, Ty};

/// The universe: three values, each its own tag, so a type over them is a plain bitmask.
const VALUES: usize = 3;
const TAGS: [Tag; VALUES] = [Tag::Int, Tag::Bool, Tag::Str];
/// The mask naming every value — inside this model it behaves as `any`, which is the one
/// way the model is *not* faithful to the real lattice. See `is_artifact`.
const SATURATED: u8 = 0b111;

fn ty_of_mask(mask: u8) -> Ty {
    let mut ty = Ty::NEVER;
    for (bit, tag) in TAGS.iter().enumerate() {
        if mask & (1 << bit) != 0 {
            ty = ty.union(Ty::of(*tag));
        }
    }
    ty
}

/// Every function from the universe to itself, as `[f(0), f(1), f(2)]`. Twenty-seven of them.
fn functions() -> Vec<[u8; VALUES]> {
    let mut out = Vec::new();
    for first in 0..VALUES as u8 {
        for second in 0..VALUES as u8 {
            for third in 0..VALUES as u8 {
                out.push([first, second, third]);
            }
        }
    }
    out
}

/// The functions an arrow `(domain → result)` denotes: those mapping every value of the
/// domain into the result. A function is unconstrained where the domain does not reach.
fn denote_arrow(all_functions: &[[u8; VALUES]], domain: u8, result: u8) -> u32 {
    let mut set = 0u32;
    for (index, function) in all_functions.iter().enumerate() {
        let holds = (0..VALUES)
            .all(|value| domain & (1 << value) == 0 || result & (1 << function[value]) != 0);
        if holds {
            set |= 1 << index;
        }
    }
    set
}

/// One candidate type: one arrow, or two arrows intersected.
#[derive(Clone, Copy)]
struct Candidate {
    arrows: [(u8, u8); 2],
    count: usize,
}

impl Candidate {
    /// The function set this denotes — an intersection, so the sets are AND-ed.
    fn denotation(&self, all_functions: &[[u8; VALUES]]) -> u32 {
        self.arrows[..self.count]
            .iter()
            .fold(!0u32, |acc, &(domain, result)| {
                acc & denote_arrow(all_functions, domain, result)
            })
    }

    fn to_ty(self) -> Ty {
        let sigs: Vec<Sig> = self.arrows[..self.count]
            .iter()
            .map(|&(domain, result)| Sig::new(vec![ty_of_mask(domain)], ty_of_mask(result)))
            .collect();
        match sigs.len() {
            1 => Ty::arrow(sigs.into_iter().next().expect("exactly one")),
            _ => Ty::overload_of(sigs),
        }
    }

    fn describe(&self) -> String {
        self.to_ty().to_string()
    }

    /// True where the finite universe misrepresents the real lattice on the *requirement*
    /// side: a result naming every tag is vacuous here — it is `any`, so every function
    /// satisfies it — while in the 23-tag lattice `int | bool | string` is a genuine
    /// constraint the checker is right to decline. An uninhabited result is the other half of
    /// the same story: such an arrow denotes no function at all, so as a left-hand side it is
    /// bottom (under everything) in a way the checker does not model.
    ///
    /// Both are properties of the model, not of the rule under test, and both are excluded
    /// from the completeness claim ONLY. Neither is excluded from the soundness claim, which
    /// every pair is held to.
    fn result_is_an_artifact(&self) -> bool {
        self.arrows[..self.count]
            .iter()
            .any(|&(_, result)| result == 0 || result == SATURATED)
    }
}

fn candidates() -> Vec<Candidate> {
    // Non-empty domains only: an empty domain constrains nothing and denotes every function,
    // a degenerate case the type grammar cannot write anyway.
    let arrows: Vec<(u8, u8)> = (1u8..=SATURATED)
        .flat_map(|domain| (0u8..=SATURATED).map(move |result| (domain, result)))
        .collect();
    let mut out: Vec<Candidate> = arrows
        .iter()
        .map(|&arrow| Candidate {
            arrows: [arrow, arrow],
            count: 1,
        })
        .collect();
    for (index, &first) in arrows.iter().enumerate() {
        for &second in &arrows[index + 1..] {
            out.push(Candidate {
                arrows: [first, second],
                count: 2,
            });
        }
    }
    out
}

#[test]
fn arrow_subtyping_agrees_with_what_an_arrow_denotes() {
    let all_functions = functions();
    // Build each candidate's `Ty` and denotation ONCE. Doing it inside the inner loop costs
    // 1596 rebuilds of every one of them, which is most of the run and is pure contention
    // against whatever else the test runner is doing.
    let cands: Vec<(Candidate, Ty, u32)> = candidates()
        .into_iter()
        .map(|cand| {
            let denotation = cand.denotation(&all_functions);
            (cand, cand.to_ty(), denotation)
        })
        .collect();
    let mut unsound: Vec<String> = Vec::new();
    let mut missed: Vec<String> = Vec::new();
    let mut compared = 0usize;
    let mut judgeable = 0usize;
    for (left, left_ty, left_set) in &cands {
        for (right, right_ty, right_set) in &cands {
            compared += 1;
            let claimed = left_ty.is_subtype(right_ty);
            // Plain subset containment of the denoted function sets — no checker rules here.
            let truth = left_set & !right_set == 0;
            if claimed && !truth {
                unsound.push(format!(
                    "  `{}` <: `{}` was claimed, but a function is in the first and not the \
                     second",
                    left.describe(),
                    right.describe()
                ));
            }
            if *left_set == 0 || right.result_is_an_artifact() {
                continue;
            }
            judgeable += 1;
            if truth && !claimed {
                missed.push(format!(
                    "  `{}` <: `{}` holds, but was not proven",
                    left.describe(),
                    right.describe()
                ));
            }
        }
    }

    let sample = |lines: &[String]| lines.iter().take(8).cloned().collect::<Vec<_>>().join("\n");
    assert!(
        unsound.is_empty(),
        "arrow subtyping accepted {} containment(s) the semantics forbid — this is the error \
         the lattice must never make:\n{}",
        unsound.len(),
        sample(&unsound)
    );
    // Completeness is asserted too, not merely reported: on everything this model can
    // legitimately judge the rule is exact, so any new gap is a real loss of precision and
    // worth a conversation. If a future fix must decline more in order to stay sound, that is
    // the right trade — take it, and record here what it costs.
    assert!(
        missed.is_empty(),
        "arrow subtyping failed to prove {} containment(s) that hold, out of {judgeable} \
         judgeable pairs (it proved all of them before):\n{}",
        missed.len(),
        sample(&missed)
    );
    // The enumeration must actually be exercised — a pairing that compared nothing would
    // satisfy both assertions above while proving nothing at all (ADR-280).
    assert!(
        compared > 2_000_000 && judgeable > 500_000,
        "only {compared} pairs compared ({judgeable} judgeable) — the enumeration is not \
         covering what it looks like"
    );
}
