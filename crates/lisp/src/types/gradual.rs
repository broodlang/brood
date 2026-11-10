//! The gradual type [`GradualTy`] (ADR-024) — a static bound plus the `dynamic()` flag —
//! with consistent subtyping derived from set inclusion, and the process-wide strict
//! checking switch that reads it by inclusion instead of by overlap.

use super::*;

/// A **gradual** type — `dynamic()` brought *inside* the lattice (ADR-024,
/// `docs/types.md`), not a bolt-on. It is a static [`Ty`] `bound` plus a
/// `dynamic` flag: flag clear → exactly the static set; flag set →
/// `dynamic(bound)`, "materialisable to anything within `bound`". Pure
/// `dynamic()` is `dynamic(ANY)`.
///
/// The defining property: **consistent subtyping is *derived from* set
/// inclusion**, never a separate consistency axiom (the classic Siek–Taha
/// bolt-on — see ADR-024). A value flows where a static `t` is expected iff a
/// static type does (`bound ⊆ t`) or — when dynamic — *some* inhabited
/// materialisation fits (`bound ∩ t ≠ ⊥`). So pure `dynamic()` is consistent with
/// every inhabited type (defer the check), while `dynamic(number)` is still
/// caught against `string`.
///
/// **The rule (no checker consumes it yet):** anything whose static type can't be
/// pinned — above all a *redefinable global under hot reload* — is `dynamic()`,
/// never `ANY`. (`ANY` relates by subtyping and would error where an `int` is
/// wanted; `dynamic()` defers, which is what lets typing coexist with live
/// redefinition.)
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GradualTy {
    /// What we statically know: every materialisation is `⊆ bound`.
    pub bound: Ty,
    /// Whether the gradual `?` is in play (materialisable within `bound`).
    pub dynamic: bool,
}

/// The process-wide strict switch. `nest check --strict` flips it before it runs the
/// checker; with nothing set explicitly it is `BROOD_CHECK_STRICT=1`, read once — so
/// EVERY entry point (`brood --check`, the REPL's advisory check, the LSP,
/// `check-string-here`) honours the env spelling, not only the one command that used to
/// read it itself (2026-09-12: the flag catalogue and ADR-298 named it as the mode switch,
/// and `brood --check` with it set reported the plain verdict). A file check reads the
/// mode once, at its root (`check_file_ext`), into the checker context — so it is a
/// launch setting, never something the walk consults.
///
/// `0` = not set explicitly (the env decides), `1` = off, `2` = on.
static STRICT_CHECKING: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static STRICT_FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Turn strict checking on or off for this process — see
/// [`GradualTy::consistent_with_mode`]. An explicit setting wins over the env.
pub fn set_strict_checking(on: bool) {
    STRICT_CHECKING.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
}

/// Is strict checking on for this process?
pub fn strict_checking() -> bool {
    match STRICT_CHECKING.load(std::sync::atomic::Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => *STRICT_FROM_ENV
            .get_or_init(|| std::env::var_os("BROOD_CHECK_STRICT").is_some_and(|v| v == "1")),
    }
}

impl GradualTy {
    /// A purely static gradual type — exactly the set `t`, no `?`.
    pub const fn stat(t: Ty) -> GradualTy {
        GradualTy {
            bound: t,
            dynamic: false,
        }
    }

    /// `dynamic(bound)` — gradual, materialisable to anything within `bound`.
    pub const fn dynamic_within(bound: Ty) -> GradualTy {
        GradualTy {
            bound,
            dynamic: true,
        }
    }

    /// Pure `dynamic()` = `dynamic(ANY)` — the unknown type a redefinable global
    /// or free reference gets, so checking never fights hot reload.
    pub const fn dynamic() -> GradualTy {
        GradualTy::dynamic_within(Ty::ANY)
    }

    /// Is the gradual `?` in play?
    pub const fn is_dynamic(&self) -> bool {
        self.dynamic
    }

    /// **Consistent subtyping** into a static expectation — derived from set
    /// inclusion, the relation a checker uses for "can a value of this gradual
    /// type be used where `expected` is wanted?". Static: `bound ⊆ expected`.
    /// Dynamic: some inhabited materialisation fits, `bound ∩ expected ≠ ⊥`.
    pub fn consistent_with(&self, expected: Ty) -> bool {
        self.consistent_with_mode(expected, false)
    }

    /// [`consistent_with`](Self::consistent_with) with the **strict** switch (`nest check
    /// --strict`). Strict mode keeps the dynamic reading only for the genuinely unknown —
    /// a bare `dynamic()` (`any`) — and checks a dynamic value whose bound is anything
    /// narrower by inclusion, `bound ⊆ expected`, exactly like a static one. That is the
    /// "warn on the merely-wider" precision the gradual overlap rule deliberately gives
    /// up for reload-safety (docs/type-gating.md, B1): a `number` handed to an `int`
    /// parameter is *consistent* by overlap, and *rejected* strictly. Inclusion here is
    /// [`Ty::is_consistent_subtype`]: a NESTED unknown (a record field, an element) is
    /// the gradual `?` in either mode — only what is positively known is read strictly.
    pub fn consistent_with_mode(&self, expected: Ty, strict: bool) -> bool {
        if !self.dynamic {
            // A PRECISE bound — a literal, a sig-typed parameter — is read by inclusion, term
            // by term; but an arm known only by exclusion is the unknown even when it came
            // from a declaration: `(sig f (any -> …))` then `(when xs (first xs))` leaves
            // `xs : (not (nil | false))`, which says nothing positive, and reading it by
            // inclusion reported the guarded use of a declared-`any` parameter in both
            // modes. Declaring `any` means what not declaring means.
            return self.bound.terms_vec().into_iter().all(|term| {
                if term.is_known_only_by_exclusion() {
                    !term.is_disjoint(&expected)
                } else {
                    term.is_consistent_subtype(&expected)
                }
            });
        }
        // A dynamic bound is read TERM BY TERM. A union that puts a positively-known
        // alternative beside one known only by exclusion — `nil | list<int> | (not (nil |
        // vector))`, what a `cond` that returns its scrutinee in the `else` arm yields —
        // is not positively known as a whole, and reading it as one would charge the
        // known arms with the unknown arm's admissions: the exclusion arm admits a
        // `failure` the way it admits everything, and strict inclusion would demand that
        // "anything but nil or a vector" fit the parameter. So each arm gets the reading
        // its own knowledge earns, and the whole is consistent when every arm is.
        let failure = Ty::of(Tag::Failure);
        self.bound.terms_vec().into_iter().all(|term| {
            // Strict applies to a bound that is POSITIVELY known. `any ∖ nil` — what a
            // `(when x …)` guard leaves — says what the value is not, never what it is; it
            // is still the unknown, and reading it by inclusion would flag every guarded
            // use of an untyped parameter.
            let by_exclusion = term.is_known_only_by_exclusion();
            if strict && !by_exclusion {
                // The unknown keyed part (`map | record`, nothing known of either) is read
                // by overlap; the rest of the term strictly.
                if let Some((map, record, rest)) = term.keyed_kind_unknown() {
                    return (map.is_consistent_subtype(&expected)
                        || record.is_consistent_subtype(&expected))
                        && (rest.is_never() || rest.is_consistent_subtype(&expected));
                }
                return term.is_consistent_subtype(&expected);
            }
            // **A failure is never a valid materialisation** of a domain that excludes
            // one — checked by inclusion in both modes, where every other arm of a union
            // gets the overlap reading. This is the converse of the impossible-`failure?`
            // lint (ADR-315): that one names a guard that cannot fire, this one names a
            // value that can fail where nothing guards it.
            //
            // Why `failure` and not, say, `nil`, when the merely-wider rule exists
            // precisely to keep the checker out of the way (docs/type-gating.md B1): the
            // two are different channels by construction (ADR-310). `nil` means
            // **nothing** and is a legitimate result everywhere — `(get m :absent)` is an
            // answer, and reading `(or T nil)` into `T` by inclusion would fire on every
            // accessor in the language. A **failure** means a value of the expected type
            // could not be produced; it is not an answer anybody's domain accepts, and a
            // failure reaching a primitive that cannot use it RAISES (ADR-315 kept that
            // deliberately). So this reports a guaranteed error on a reachable path, not
            // a merely-wider type.
            //
            // The failure arm has to be POSITIVELY known — it comes from a declared or
            // inferred signature, i.e. someone said the function can fail. A bound known
            // only by exclusion (`any`, or a guard's `(not nil)`) admits a failure the way
            // it admits everything, and says nothing; reading that as "can fail" would
            // fire on every unannotated parameter.
            //
            // Reload safety is unaffected: a `def` that stops returning failures changes
            // the signature the next check reads, and this warning goes with it.
            if !by_exclusion && !term.is_disjoint(&failure) && expected.is_disjoint(&failure) {
                return false;
            }
            // Some inhabited materialisation fits — i.e. the arm is not *provably
            // disjoint* from `expected`. Uses [`Ty::is_disjoint`], not
            // `intersect().is_never()`: the two agree on flat tags, but only
            // `is_disjoint` also sees the refinement-level conflicts (record
            // fields, tuple shapes, literal sets), so a dynamic value with a
            // refined type that provably can't fit is caught here too.
            !term.is_disjoint(&expected)
        })
    }

    /// Gradual union — union of bounds, dynamic if either side is. Used to join
    /// the types of branches (e.g. the arms of an `if`). The static set algebra
    /// lives on [`Ty`] (`self.bound`); the only gradual combinator we expose is
    /// the one a consumer needs — gradual intersection/negation are deferred
    /// until Step 4 shows their exact semantics (ADR-011: don't ship unproven
    /// operators).
    pub fn union(self, other: GradualTy) -> GradualTy {
        GradualTy {
            bound: self.bound.union(other.bound),
            dynamic: self.dynamic || other.dynamic,
        }
    }
}
