//! The integer interval [`Range`] (ADR-350) — the refinement an `int` member and a
//! countable member's length carry — and its interval arithmetic: hull, meet, inclusion,
//! the four operations and the widening of an ascent.

/// A closed interval of integers, either end possibly unbounded (ADR-350) — the
/// refinement an `int` member carries (`int[0..]`, an index that started at zero) and
/// the LENGTH a countable member carries (`vector<int>[3]`, `list<string>[1..]`). `None`
/// at an end is that infinity. `lo > hi` is empty, and never stored: an empty meet drops
/// the tag instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub lo: Option<i64>,
    pub hi: Option<i64>,
}

impl Range {
    /// Every integer.
    pub const ALL: Range = Range { lo: None, hi: None };
    /// Exactly `n`.
    pub const fn point(n: i64) -> Range {
        Range {
            lo: Some(n),
            hi: Some(n),
        }
    }
    /// `[lo, ∞)`.
    pub const fn at_least(lo: i64) -> Range {
        Range {
            lo: Some(lo),
            hi: None,
        }
    }
    /// `(-∞, hi]`.
    pub const fn at_most(hi: i64) -> Range {
        Range {
            lo: None,
            hi: Some(hi),
        }
    }
    pub const fn new(lo: Option<i64>, hi: Option<i64>) -> Range {
        Range { lo, hi }
    }
    pub fn is_all(&self) -> bool {
        self.lo.is_none() && self.hi.is_none()
    }
    pub fn is_empty(&self) -> bool {
        matches!((self.lo, self.hi), (Some(l), Some(h)) if l > h)
    }
    /// The one value, when there is exactly one.
    pub fn as_point(&self) -> Option<i64> {
        match (self.lo, self.hi) {
            (Some(l), Some(h)) if l == h => Some(l),
            _ => None,
        }
    }
    /// The smallest range holding both — the union's WIDENING (`[0,0] ∪ [5,∞)` admits 3).
    pub fn hull(a: Range, b: Range) -> Range {
        Range {
            lo: match (a.lo, b.lo) {
                (Some(x), Some(y)) => Some(x.min(y)),
                _ => None,
            },
            hi: match (a.hi, b.hi) {
                (Some(x), Some(y)) => Some(x.max(y)),
                _ => None,
            },
        }
    }
    /// The intersection, or `None` when it is empty.
    pub fn meet(a: Range, b: Range) -> Option<Range> {
        let out = Range {
            lo: match (a.lo, b.lo) {
                (Some(x), Some(y)) => Some(x.max(y)),
                (x, None) => x,
                (None, y) => y,
            },
            hi: match (a.hi, b.hi) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (x, None) => x,
                (None, y) => y,
            },
        };
        (!out.is_empty()).then_some(out)
    }
    pub fn subset(a: Range, b: Range) -> bool {
        let lo_ok = match (a.lo, b.lo) {
            (_, None) => true,
            (Some(x), Some(y)) => x >= y,
            (None, Some(_)) => false,
        };
        let hi_ok = match (a.hi, b.hi) {
            (_, None) => true,
            (Some(x), Some(y)) => x <= y,
            (None, Some(_)) => false,
        };
        lo_ok && hi_ok
    }
    pub fn disjoint(a: Range, b: Range) -> bool {
        Range::meet(a, b).is_none()
    }
    /// Interval arithmetic: `[a,b] + [c,d] = [a+c, b+d]`, an unbounded end stays
    /// unbounded, and an end that overflows `i64` WIDENS to unbounded (ADR-350) — it
    /// never saturates. A saturated end is a wrong bound, not a loose one: `(+ i64::MAX
    /// 1)` saturated to `i64::MAX`, so `(- (+ i64::MAX 1) i64::MAX)` read as exactly 0
    /// and a `(-> (int 1 1))` declaration was reported as violated.
    pub fn plus(a: Range, b: Range) -> Range {
        Range {
            lo: a.lo.zip(b.lo).and_then(|(x, y)| x.checked_add(y)),
            hi: a.hi.zip(b.hi).and_then(|(x, y)| x.checked_add(y)),
        }
    }
    /// `-[a,b] = [-b, -a]`; negating `i64::MIN` overflows, so that end widens.
    pub fn negated(a: Range) -> Range {
        Range {
            lo: a.hi.and_then(i64::checked_neg),
            hi: a.lo.and_then(i64::checked_neg),
        }
    }
    pub fn minus(a: Range, b: Range) -> Range {
        Range::plus(a, Range::negated(b))
    }
    /// `[a,b] × [c,d]` — the four corner products when every end is finite; else, the
    /// sign is all that survives (both non-negative → `[0, ∞)`), else everything. A
    /// corner product that overflows widens the end on its side (a positive overflow the
    /// upper end, a negative one the lower) rather than saturating — see [`plus`].
    ///
    /// [`plus`]: Range::plus
    pub fn times(a: Range, b: Range) -> Range {
        match (a.lo, a.hi, b.lo, b.hi) {
            (Some(al), Some(ah), Some(bl), Some(bh)) => {
                let corners = [(al, bl), (al, bh), (ah, bl), (ah, bh)];
                let mut lo = Some(i64::MAX);
                let mut hi = Some(i64::MIN);
                for (x, y) in corners {
                    match x.checked_mul(y) {
                        Some(product) => {
                            lo = lo.map(|end| end.min(product));
                            hi = hi.map(|end| end.max(product));
                        }
                        // the true product is past the edge on the side its sign says
                        None if (x < 0) == (y < 0) => hi = None,
                        None => lo = None,
                    }
                }
                Range { lo, hi }
            }
            _ => {
                let non_negative = |r: Range| r.lo.is_some_and(|l| l >= 0);
                if non_negative(a) && non_negative(b) {
                    Range::at_least(0)
                } else {
                    Range::ALL
                }
            }
        }
    }
    /// The WIDENING of an ascent: an end that moved outward since `prev` goes to its
    /// infinity, so a loop counter's `[0,0], [0,1], [0,2], …` is `[0, ∞)` in one step.
    pub fn widen(prev: Range, next: Range) -> Range {
        Range {
            lo: match (prev.lo, next.lo) {
                (Some(p), Some(n)) if n < p => None,
                (None, _) => None,
                (_, n) => n,
            },
            hi: match (prev.hi, next.hi) {
                (Some(p), Some(n)) if n > p => None,
                (None, _) => None,
                (_, n) => n,
            },
        }
    }
}
