//! Function type signatures — [`Sig`] (extracted from mod.rs).
use super::*;

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
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Sig {
    /// The fixed positional argument types, in order.
    pub params: Vec<Ty>,
    /// Optional positional argument types, following `params` — present iff
    /// the corresponding argument is supplied (`&optional` in `(sig …)`
    /// grammar, mirroring a closure's `&optional` params). Empty for a sig
    /// with no optional params (every pre-existing constructor).
    pub optional: Vec<Ty>,
    /// The variadic-tail type — applies to every argument beyond `params` +
    /// `optional`. `None` means no rest (extras are an arity error, caught
    /// separately).
    pub rest: Option<Ty>,
    /// The result type.
    pub ret: Ty,
}

impl Sig {
    /// `params -> ret` — fixed arity, no rest tail.
    pub fn new(params: Vec<Ty>, ret: Ty) -> Sig {
        Sig {
            params,
            optional: Vec::new(),
            rest: None,
            ret,
        }
    }
    /// `() -> ret` — a nullary primitive (a thunk / accessor).
    pub fn nullary(ret: Ty) -> Sig {
        Sig {
            params: Vec::new(),
            optional: Vec::new(),
            rest: None,
            ret,
        }
    }
    /// `(...rest) -> ret` — pure variadic, every argument is `rest`.
    pub fn variadic(rest: Ty, ret: Ty) -> Sig {
        Sig {
            params: Vec::new(),
            optional: Vec::new(),
            rest: Some(rest),
            ret,
        }
    }
    /// `params... ...rest -> ret` — fixed leading params then a variadic tail.
    pub fn with_rest(params: Vec<Ty>, rest: Ty, ret: Ty) -> Sig {
        Sig {
            params,
            optional: Vec::new(),
            rest: Some(rest),
            ret,
        }
    }
    /// `params... &optional optional... -> ret` — fixed params then optional
    /// ones, no rest tail.
    pub fn with_optional(params: Vec<Ty>, optional: Vec<Ty>, ret: Ty) -> Sig {
        Sig {
            params,
            optional,
            rest: None,
            ret,
        }
    }
    /// `params... &optional optional... & rest -> ret` — all three parameter
    /// kinds together, mirroring a closure's full `(req &optional opt & rest)`
    /// shape.
    pub fn with_optional_and_rest(params: Vec<Ty>, optional: Vec<Ty>, rest: Ty, ret: Ty) -> Sig {
        Sig {
            params,
            optional,
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
    /// The type expected at argument position `i` — fixed params, then
    /// `optional` params, then `rest` for anything beyond. `None` when too
    /// many args are passed for a non-variadic sig (a separate arity check
    /// catches that).
    pub fn param(&self, i: usize) -> Option<Ty> {
        self.params
            .get(i)
            .cloned()
            .or_else(|| self.optional.get(i - self.params.len()).cloned())
            .or_else(|| self.rest.clone())
    }

    /// Arrow subtyping `self <: other` — a function of type `self` is usable
    /// wherever `other` is expected. **Contravariant in parameters** (`self` must
    /// accept everything `other` might pass: `other.param(i) <: self.param(i)`)
    /// and **covariant in the result** (`self.ret <: other.ret`). Arities must
    /// be compatible. Used by [`Ty::is_subtype`] for the function members and by
    /// the checker's callback compatibility step.
    pub fn is_subtype(&self, other: &Sig) -> bool {
        // Result: covariant.
        if !self.ret.is_subtype(&other.ret) {
            return false;
        }
        // Arity must line up: a fixed-arity `self` can't satisfy an `other` that
        // may pass more (or fewer) arguments than `self` accepts.
        match (self.rest.is_some(), other.rest.is_some()) {
            (false, true) => return false, // other is variadic, self isn't
            (false, false) => {
                // `other`'s achievable arity range (its required count up to
                // required+optional) must sit inside `self`'s. With no
                // `optional` on either side this is exactly the original
                // `params.len() != params.len()` equality check — verified
                // equivalent, so pre-existing (optional-free) sigs compare
                // identically to before.
                let self_max = self.params.len() + self.optional.len();
                let other_max = other.params.len() + other.optional.len();
                if self.params.len() > other.params.len() || self_max < other_max {
                    return false;
                }
            }
            // The remaining cases — `(true, _)`: a variadic `self` — are not
            // rejected here; their arity compatibility is checked positionally by
            // the param loop below, which iterates max(len) positions and uses
            // `param(i)` (folding `rest` in), so a variadic `self` is required to
            // accept every argument `other` may supply.
            _ => {}
        }
        // Parameters: contravariant — for every position `other` may supply,
        // `self` must accept at least as much. The bound includes each side's
        // `optional` positions too (empty for a sig with none, same bound as
        // before).
        let arity = (self.params.len() + self.optional.len())
            .max(other.params.len() + other.optional.len());
        for i in 0..arity {
            match (other.param(i), self.param(i)) {
                (Some(o), Some(s)) => {
                    if !o.is_subtype(&s) {
                        return false;
                    }
                }
                // `other` supplies an argument `self` has no parameter for.
                (Some(_), None) => return false,
                _ => {}
            }
        }
        true
    }
}

impl fmt::Display for Sig {
    /// `(p1, p2) -> ret`, with `&optional o1, o2` for optional params, a
    /// trailing `...rest` for the variadic tail, and `()` for nullary — the
    /// arrow rendering used in diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        let mut first = true;
        for p in &self.params {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{p}")?;
        }
        if !self.optional.is_empty() {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "&optional ")?;
            let mut first_opt = true;
            for p in &self.optional {
                if !first_opt {
                    f.write_str(", ")?;
                }
                first_opt = false;
                write!(f, "{p}")?;
            }
        }
        if let Some(rest) = &self.rest {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "...{rest}")?;
        }
        write!(f, ") -> {}", self.ret)
    }
}
