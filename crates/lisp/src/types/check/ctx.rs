//! The scope/narrowing context that threads through the checker walk.
//!
//! `Ctx` is the single value the walk carries: every `let`/`if`/`fn` opens a
//! cloned-and-extended `Ctx`, and every type query bottoms out by reading
//! `Ctx`'s tables. It collects four kinds of fact:
//!
//! - **Types** (`types`) — what is each in-scope variable narrowed to right
//!   now? Populated by `let`-binding RHSs and by `if`-guards; intersected on
//!   each refinement (`narrow`).
//! - **Guard aliases** (`guards`) — a `let`-stored guard result like
//!   `(let (cond (int? x)) (if cond …))` — so the inner `if cond` narrows
//!   `x`, not the bool `cond`.
//! - **Let-binding aliases** (`aliases`) — `(let (a b) …)` makes `a` and
//!   `b` co-name the same value; narrowing either propagates to the other
//!   via BFS through this adjacency map. What makes `match`'s internal
//!   scrutinee `m__28` reach the user-visible `x`.
//! - **Locals** (`locals`) — every name introduced by a binder, regardless
//!   of whether we know its type. A fn-param is `ANY` but is in scope, so it
//!   must not be flagged "unbound".
//! - **File-globals** (`file_globals`) — names a `def`/`defn`/`defmacro`
//!   earlier in the same file introduced. The file isn't being evaluated so
//!   these aren't in `heap`'s globals; the checker tracks them itself.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::value::{Arity, Symbol};
use crate::types::{Sig, Ty};

// ---- Type-variable representation for user-declared sigs --------------------

/// A type-expression term that may contain type variables (`?A`, `?B`…).
/// Used only for user-declared `(sig …)` / `(sig! …)` forms; primitive sigs
/// are plain [`Sig`] and remain untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SigTerm {
    /// A concrete type — no variable.
    Ty(Ty),
    /// A type variable, identified by its index in the declaration
    /// (assigned sequentially at parse time: first `?`-symbol seen = 0).
    Var(u32),
    /// `(list ?A)` — the element type may be a variable.
    ListOf(Box<SigTerm>),
    /// `(vector ?A)` — the element type may be a variable.
    VectorOf(Box<SigTerm>),
    /// `(set T)` with a variable inside — `set<T>`.
    SetOf(Box<SigTerm>),
    /// `(record [&open] :k ?A …)` — a field type may be a variable. Declaration order is
    /// kept (a `Vec`, not the shape's `BTreeMap`) only for display stability; binding is
    /// by field NAME. What this exists for: `defrecord`'s constructor signature
    /// `(sig pt (?x ?y -> (record :__id__ :pt :x ?x :y ?y)))`, so that `(pt 1 2)` is the
    /// shape `{__id__: :pt, x: 1, y: 2}` at the call and `(:x (pt 1 2))` is `1` — where a
    /// concrete `x: any` in the signature made every field of every constructed record
    /// unknown.
    RecordOf {
        fields: Vec<(Symbol, SigTerm, bool)>,
        open: bool,
    },
    /// `(or ?A nil)` — a union with a variable in it, the optional-value idiom. Binds the
    /// variable to the argument MINUS the concrete alternatives (`?A ← T ∖ nil`), which is
    /// exact when one alternative is a variable and the rest are concrete; with two
    /// variables in one union nothing binds (ambiguous — they widen to `any`).
    Or(Vec<SigTerm>),
    /// `(and …)` — an intersection; every conjunct unifies against the whole argument.
    And(Vec<SigTerm>),
}

impl SigTerm {
    /// Resolve this term to a concrete `Ty` given a substitution built by
    /// unification. Unresolved variables widen to `Ty::ANY` (safe — a
    /// missing binding means no argument pinned the var).
    pub(super) fn resolve(&self, subst: &HashMap<u32, Ty>) -> Ty {
        match self {
            SigTerm::Ty(t) => t.clone(),
            SigTerm::Var(i) => subst.get(i).cloned().unwrap_or(Ty::ANY),
            SigTerm::ListOf(inner) => {
                let e = inner.resolve(subst);
                if e == Ty::ANY {
                    crate::types::Ty::LIST
                } else {
                    Ty::list_of(e)
                }
            }
            SigTerm::VectorOf(inner) => {
                let e = inner.resolve(subst);
                if e == Ty::ANY {
                    crate::types::Ty::of(crate::core::value::Tag::Vector)
                } else {
                    Ty::vector_of(e)
                }
            }
            SigTerm::SetOf(inner) => {
                let e = inner.resolve(subst);
                if e == Ty::ANY {
                    crate::types::Ty::of(crate::core::value::Tag::Set)
                } else {
                    Ty::set_of(e)
                }
            }
            SigTerm::Or(alts) => alts
                .iter()
                .map(|t| t.resolve(subst))
                .fold(Ty::NEVER, Ty::union),
            SigTerm::And(parts) => parts
                .iter()
                .map(|t| t.resolve(subst))
                .fold(Ty::ANY, Ty::intersect),
            SigTerm::RecordOf { fields, open } => {
                let resolved: std::collections::BTreeMap<Symbol, (Ty, bool)> = fields
                    .iter()
                    .map(|(name, term, required)| (*name, (term.resolve(subst), *required)))
                    .collect();
                if *open {
                    Ty::record_of_open(resolved)
                } else {
                    Ty::record_of(resolved)
                }
            }
        }
    }
}

/// A function signature whose parameters and return may contain type
/// variables.  Used exclusively for user-declared sigs; primitive sigs
/// remain plain [`Sig`].
#[derive(Clone, Debug)]
pub(super) struct SigWithVars {
    pub(super) params: Vec<SigTerm>,
    pub(super) rest: Option<SigTerm>,
    pub(super) ret: SigTerm,
}

impl SigWithVars {
    /// Build the unification substitution from a slice of argument types.
    /// Each arg's known type is unified against the corresponding param term
    /// (left-to-right, one level deep).  Binding two args to the same var
    /// unions their types (over-approximation; sound).
    pub(super) fn unify_args(&self, arg_tys: &[Option<Ty>]) -> HashMap<u32, Ty> {
        let mut subst: HashMap<u32, Ty> = HashMap::new();
        for (i, arg_ty) in arg_tys.iter().enumerate() {
            let Some(ty) = arg_ty else { continue };
            let term = if i < self.params.len() {
                &self.params[i]
            } else if let Some(r) = &self.rest {
                r
            } else {
                continue;
            };
            unify_term(term, ty.clone(), &mut subst);
        }
        subst
    }

    /// Resolve the return type given the argument types.
    pub(super) fn resolve_ret(&self, arg_tys: &[Option<Ty>]) -> Ty {
        let subst = self.unify_args(arg_tys);
        self.ret.resolve(&subst)
    }
}

/// Resolve a call's return type against a **declared overload** (ADR-116) —
/// the per-clause counterpart of [`SigWithVars::resolve_ret`]. A candidate
/// `sig` matches when every argument whose type is *known* is a subtype of
/// `sig`'s parameter at that position (`Sig::param`, which already folds a
/// variadic `rest` in); an unknown arg never rules a candidate out. Every
/// matching candidate's `ret` is unioned — exactly one match gives the exact
/// per-clause return type, several gives a sound (if less precise) superset,
/// and **zero matches widens to `Ty::ANY`** rather than ever fabricating a
/// return type for a call that fits no declared arm. See
/// `docs/type-arrow-intersection.md`.
pub(super) fn resolve_overload_ret(sigs: &[Sig], arg_tys: &[Option<Ty>]) -> Ty {
    let mut matched: Option<Ty> = None;
    for sig in sigs {
        let compatible = arg_tys.iter().enumerate().all(|(i, arg_ty)| {
            let Some(arg_ty) = arg_ty else {
                return true; // unknown arg type never rules a candidate out
            };
            match sig.param(i) {
                Some(param_ty) => arg_ty.is_subtype(&param_ty),
                None => false, // more args than this candidate (non-variadic) accepts
            }
        });
        if compatible {
            matched = Some(match matched {
                Some(acc) => acc.union(sig.ret.clone()),
                None => sig.ret.clone(),
            });
        }
    }
    matched.unwrap_or(Ty::ANY)
}

/// Unify a single `SigTerm` against a known concrete `ty`, extending `subst`.
/// One level deep — no recursive types.
pub(super) fn unify_term(term: &SigTerm, ty: Ty, subst: &mut HashMap<u32, Ty>) {
    match term {
        SigTerm::Ty(_) => {} // concrete: nothing to bind
        SigTerm::Var(i) => {
            let entry = subst.entry(*i).or_insert(Ty::NEVER);
            *entry = entry.clone().union(ty);
        }
        SigTerm::ListOf(inner) => {
            if let Some(elem) = ty.elem_ty() {
                unify_term(inner, elem.clone(), subst);
            }
        }
        SigTerm::VectorOf(inner) => {
            if let Some(elem) = ty.elem_ty() {
                unify_term(inner, elem.clone(), subst);
            }
        }
        SigTerm::SetOf(inner) => {
            if let Some(elem) = ty.elem_ty() {
                unify_term(inner, elem.clone(), subst);
            }
        }
        SigTerm::Or(alts) => {
            let vars: Vec<&SigTerm> = alts
                .iter()
                .filter(|t| !matches!(t, SigTerm::Ty(_)))
                .collect();
            if vars.len() == 1 {
                let concrete = alts
                    .iter()
                    .filter_map(|t| match t {
                        SigTerm::Ty(c) => Some(c.clone()),
                        _ => None,
                    })
                    .fold(Ty::NEVER, Ty::union);
                unify_term(vars[0], ty.difference(concrete), subst);
            }
        }
        SigTerm::And(parts) => {
            for part in parts {
                unify_term(part, ty.clone(), subst);
            }
        }
        SigTerm::RecordOf { fields, .. } => {
            // Bind each variable field from the argument's shape, by name. An argument
            // that is not a record (or lacks the field) binds nothing — the var widens to
            // `any` at resolve, which is the conservative reading.
            if let Some(shape) = ty.record_fields() {
                for (name, term, _) in fields {
                    if let Some((fty, _)) = shape.get(name) {
                        unify_term(term, fty.clone(), subst);
                    }
                }
            }
        }
    }
}

/// Locally-known types for variables in scope — populated by `let`/`let*`
/// bindings and by an enclosing `if`'s guard. Globals are never tracked here
/// (they're redefinable under hot reload — `dynamic()`, not `Any`).
///
/// `Ty::ANY` and "absent" both mean "no useful info"; we keep absent variables
/// out of the map so the printer in tests stays uncluttered.
///
/// **Guard aliases.** When a `let` binds a name to a recognised guard call —
/// `(let (cond (int? x)) (if cond …))` — we also remember that the bound name
/// *is* the result of testing that variable, so the inner `if cond` can
/// narrow `x` (not the bool `cond` itself). The aliasing is sound because
/// Brood is immutable: between the let and the if, neither `x` nor `cond` can
/// change, so the assertion the guard recorded still applies.
/// Advisory-lint categories a `(check-allow …)` marker can suppress for the
/// subtree it wraps. A cheap `u8` bitset so it copies with every `Ctx` clone
/// (`narrow`/`bind` clone the ctx per branch); the whole checker needs only a
/// couple of bits. See `docs/type-annotations.md` and the `%lint-allow` handling
/// in `walk.rs` / `recursion.rs`.
pub(super) const SUPPRESS_NON_TAIL: u8 = 1 << 0;
pub(super) const SUPPRESS_UNREACHABLE: u8 = 1 << 1;
/// A declared-vs-actual type mismatch the checker would otherwise warn on: a
/// `sig`-typed function whose body yields a type disjoint from its declared
/// return, or a literal call-site argument disjoint from the parameter's
/// declared type. Deliberately-wrong code under test (a negative test proving a
/// `sig!` runtime contract throws) suppresses it with `(check-allow :type-mismatch …)`.
pub(super) const SUPPRESS_TYPE_MISMATCH: u8 = 1 << 2;
/// `(check-allow :unbound …)` — the wrapped forms reference globals the checker
/// cannot see because they are defined at *runtime* (`eval`-driven `def`s: the
/// wasm `use-native` binding, a plugin loader). The one lint whose ground truth
/// is the live image, not the source.
pub(super) const SUPPRESS_UNBOUND: u8 = 1 << 3;
/// `(check-allow :unrequired …)` — a qualified reference `mod/name` whose module the
/// file never `require`s/`:use`s (KI-17). Suppresses the load-order-reachability lint
/// for a file that deliberately relies on an ambient require pulled in elsewhere.
pub(super) const SUPPRESS_UNREQUIRED: u8 = 1 << 4;
/// `(check-allow :deprecated …)` — a use of a name a `(meta … :deprecated …)` marks
/// (ADR-283). A library must sometimes call its own deprecated name from the shim that
/// replaces it, and a test must sometimes exercise the old surface deliberately.
pub(super) const SUPPRESS_DEPRECATED: u8 = 1 << 5;

/// One step of a narrowable access path: a keyword field (`(get x :k)`) or a
/// fixed integer index (`(nth x 0)` / `(first x)` / `(second x)` / `(third x)`).
/// A path is a base symbol plus a chain of these — the key of a path narrowing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum PathKey {
    Field(Symbol),
    Index(usize),
}

#[derive(Clone, Default)]
pub(super) struct Ctx {
    /// Strict mode (`nest check --strict`): a dynamic value with a PRECISE bound is
    /// checked by inclusion, not overlap — see [`crate::types::GradualTy::consistent_with_mode`].
    strict_mode: bool,
    /// Lint categories suppressed in the current subtree (a `(check-allow …)`
    /// scope, ORed as we descend). `0` = nothing suppressed (the common case).
    suppressed: u8,
    /// Ability facts for the file (op-fn symbols → `(ability, op)`, and the covered
    /// impls) — set once at the file level so `check_into` can flag an ability op applied
    /// to a record-typed value with no impl. `None` in a file with no `defability`.
    /// Shared (`Arc`) so cloning `Ctx` as the walk descends stays cheap.
    ability: Option<std::sync::Arc<super::protocol::AbilityInfo>>,
    /// Multimethod facts for the file (`defmulti` generic symbols + covered tuples) — set
    /// once at the file level so `check_into` can flag a generic call whose args' identities
    /// come from *inference* (a record-typed variable), complementing the syntactic pass.
    /// `None` in a file with no `defmulti`. Shared (`Arc`) so cloning `Ctx` stays cheap.
    multi: Option<std::sync::Arc<super::protocol::MultiInfo>>,
    /// The function whose signature is being **inferred** (`sigs::infer_sig`), if any. A
    /// self-recursive call to it in a branch-result position contributes ⊥ to the return
    /// union — by induction it returns the same as the base cases, so skipping it lets a
    /// recursive function's return infer from its non-recursive branches. `None` outside
    /// return inference. (Set only on the return-inference `Ctx`, never the file walk.)
    inferring_self: Option<Symbol>,
    types: HashMap<Symbol, Ty>,
    /// **Path narrowings** — the type of a *compound path* asserted by an
    /// enclosing guard, keyed by a base symbol plus a chain of [`PathKey`]s
    /// (keyword fields and/or fixed indices). Occurrence typing through
    /// (possibly nested) field / index access: `(if (int? (get r :age)) …)`
    /// records `(r, [Field :age]) → int`, and `(if (int? (nth (get cfg :items) 0)) …)`
    /// records `(cfg, [Field :items, Index 0]) → int`, for the then-branch. Sound
    /// because Brood is immutable — neither the base nor the pure access chain can
    /// change between the guard and a use, so the assertion holds. Consulted by
    /// `guards::expr_ty`'s path lookup; empty in the common (no-guard) case.
    path_types: HashMap<(Symbol, Vec<PathKey>), Ty>,
    /// `bound-name → (variable, type-it-asserts)`: a `let`-stored guard result.
    guards: HashMap<Symbol, (Symbol, Ty)>,
    /// **Let-binding aliases.** `(let (a b) …)` aliases `a` and `b` — they
    /// name the same value through the scope, so narrowing either propagates
    /// to the other. Stored as an undirected adjacency map (each name maps
    /// to its co-equivalent set), so `narrow` BFSes the equivalence class
    /// and tightens every member. Brood is immutable, so the relation is
    /// sound for the binding's extent; `bind` (shadow) disconnects the name
    /// from every neighbour to prevent stale aliasing across re-bindings.
    aliases: HashMap<Symbol, HashSet<Symbol>>,
    /// Every locally-bound name in scope — fn/lambda params and let bindings.
    /// Distinct from `types`: a fn-param has *no known type* (`ANY` by default)
    /// but is *in scope*, so it must not be flagged unbound. `types` records
    /// narrowings on top; `locals` records existence.
    locals: HashSet<Symbol>,
    /// Top-level names defined earlier in the same file (`def`/`defn`/
    /// `defmacro` accumulated by [`check_file`]). The file isn't being
    /// evaluated, so these aren't in `heap`'s global table — we track them
    /// here so a later form doesn't flag them as unbound.
    file_globals: HashSet<Symbol>,
    /// File-local `(defmacro name …)` names. A subset of `file_globals` tagged as
    /// *macros* specifically, so the walk can recognise a call whose head is a
    /// file-local macro it couldn't expand (single-file mode, or a macro defined
    /// inside a deferred `test`/`describe` thunk) and treat its argument syntax as
    /// opaque — a macro may quote its args, splice them into a binder, or `def` a
    /// symbol arg, so walking them as evaluated code would false-flag. Populated by
    /// [`collect_def_names`](super::walk::collect_def_names).
    file_macros: HashSet<Symbol>,
    /// File-local `def`/`defn` names whose value is a **variadic** `fn` (a `&`
    /// rest param). The declared `(sig …)` parser only builds fixed-arity sigs,
    /// so a sig on a variadic defn would otherwise yield a spurious *exact* arity
    /// (`Arity::exact(sig.params.len())`) and a false "wrong number of arguments"
    /// warning when called with more args. Recording the def site's real
    /// variadic-ness lets the arity check suppress the sig-derived exact arity —
    /// preserving the advisory no-false-positives rule. Populated by
    /// [`collect_def_names`](super::walk::collect_def_names).
    variadic_globals: HashSet<Symbol>,
    /// File-local `def`/`defn` names → the arity their **definition** admits, read
    /// off the def site's own parameter list ([`collect_def_names`](super::walk::collect_def_names)).
    /// The file isn't loaded while it is checked, so `sigs::arity_of` (which reads the
    /// global table) sees nothing for a same-file name — before this, a call to a function
    /// defined in the same file had **no arity check at all**, and a `(sig …)` that
    /// disagreed with the definition silently supplied a wrong one. Multi-arm closures
    /// store the interval hull (smallest min, largest max), exactly as `arity_of` does for
    /// a loaded closure: sound (over-accepting), never a false positive. Absent for a
    /// def whose value isn't a `fn` form, or whose parameter list the checker can't read
    /// (a destructuring binder) — the check then stays silent, as before.
    file_arity: HashMap<Symbol, Arity>,
    /// `(sig name (… -> …))` declarations — authoritative signatures the user
    /// wrote, read *first* by the call-checker (ahead of primitive/curated/
    /// inferred). Populated by [`check_file`]'s scan of the un-expanded forms.
    /// Slice 1 trusts these without runtime enforcement; slice 2 (the strong
    /// arrow) makes that trust sound. See `docs/type-annotations.md`.
    /// **Inferred** per-arm signatures for a same-file *multi-arm* function — the
    /// inferred counterpart of [`declared_overload`](Ctx::declared_overload). A
    /// multi-arm closure has no single `Sig`, so its callers' arguments went
    /// unchecked; each arm has one, and a call no arity-relevant arm accepts is a
    /// provable error. Populated by `check_file`'s Pass 2.8.
    inferred_overload: HashMap<Symbol, Vec<Sig>>,
    declared: HashMap<Symbol, Sig>,
    /// `(sig x T)` declarations for **value** names (non-arrow types) — `x : int`.
    /// The gradual-assignment check reads these to verify a `(def x <expr>)`
    /// assigns a value consistent with `T` (via [`GradualTy::consistent_with`]).
    /// Separate from [`declared`] (function arrow sigs); a name has at most one.
    declared_value_ty: HashMap<Symbol, Ty>,
    /// **Inferred** value types for undeclared globals defined exactly once by a
    /// `(def g <non-fn-expr>)` (Gap A, `docs/type-gating.md`). The type of the RHS,
    /// used as a *current-image* observation — always exposed as `dynamic_within`
    /// (the `∩` relation), never a precise `stat`, because a global is redefinable
    /// (a reload re-derives it; ADR-125). Read *after* [`declared_value_ty`], which
    /// is authoritative. Only populated for globals defined exactly once (a
    /// redefined global's type is ambiguous — it stays `dynamic()`).
    inferred_value_ty: HashMap<Symbol, Ty>,
    /// **Same-file inferred function signatures** — the sig the checker inferred for a
    /// `(defn …)` in *this* file, from its form (the file isn't loaded while it's checked, so
    /// `sigs::sig_of`'s loaded-closure inference can't see it). Populated by `check_file`'s
    /// fixpoint pass and read at a call site *after* a declared sig (which is authoritative),
    /// so a same-file caller gets the same checking a cross-module caller of a loaded function
    /// already got. Return-only for now (params-less), so it flows results without imposing
    /// argument constraints. Redefinable-global caution is the caller's (treated as an
    /// over-approximation, like the loaded-inferred sigs).
    inferred_fn_sig: HashMap<Symbol, Sig>,
    /// User-declared sigs that contain type variables (`?A`) — the full
    /// [`SigWithVars`] for unification at call sites.  Populated alongside
    /// [`declared`] when the sig annotation has at least one `?`-symbol.
    /// `declared` always carries the flattened version (`?A` → `Ty::ANY`) so
    /// the arity-fallback path is unchanged; this table carries the richer form.
    declared_vars: HashMap<Symbol, SigWithVars>,
    /// User-declared sigs whose type-expr is an **overload** — `(and (int ->
    /// int) (bool -> bool))`, 2+ distinct arrows (ADR-116). Populated
    /// alongside [`declared`] instead of it (an overloaded sig has no single
    /// `Sig` — [`annot::parse_sig_decl`] can't produce one, since `Ty::as_arrow()`
    /// is `None` for a genuine overload). Read by [`resolve_overload_ret`] to
    /// give a call site the *matching arm's* return type instead of a flat
    /// fallback. See `docs/type-arrow-intersection.md`.
    declared_overloads: HashMap<Symbol, Vec<Sig>>,
    /// Parameters whose type was **seeded from the enclosing function's `(sig …)`
    /// declaration** — the subset of `types` we trust enough to flag a *dead
    /// clause* on. A guard that narrows one of these to the empty type means a
    /// `match`/`cond` clause can never run (the declared type is incompatible
    /// with the pattern). Gating on this set is what keeps the dead-clause lint
    /// free of false positives: a literal scrutinee or a compiler-generated guard
    /// (destructure / `match` lowering) never involves a sig-typed param, so it
    /// is never flagged. Shadowing removes a name (see [`bind`](Ctx::bind)).
    sig_params: HashSet<Symbol>,
    /// **Surface `let`-locals eligible for the dead-clause lint** — the broadening
    /// of that lint past sig-typed params. A `let`-bound local qualifies only when
    /// its RHS has a **precise** (`GradualTy.dynamic == false`) type — a literal or
    /// integer-closed expression, never a call-result or redefinable-global
    /// reference (those are `dynamic`, so a "dead" conclusion could be invalidated
    /// by a reload — excluding them keeps the lint reload-safe) — and its name is
    /// **surface** (not a gensym temp from macro expansion) with a source position.
    /// A local is immutable within its scope, so an over-approximated-but-precise
    /// type narrowed to `never` by a guard proves the branch dead, exactly as a
    /// sig-param does. Shadowing removes a name (see [`bind`](Ctx::bind)).
    dead_clause_locals: HashSet<Symbol>,
    /// Whether to flag *operand / value-slot* unbound symbols (a bare symbol in
    /// an evaluated argument or a `def`/`let`/`if` value position). On only when
    /// checking a **complete file** ([`check_file`]): there every top-level def
    /// is in `file_globals` and the project image is loaded, so an unresolved
    /// operand is genuinely unbound. Off for a bare fragment ([`check_form`] /
    /// the `(check 'form)` builtin / REPL snippets), where a free variable is
    /// legitimately ambiguous (a surrounding-scope or REPL global), so flagging
    /// it would be a false positive. Call *heads* are flagged in both modes —
    /// an unbound callee is reliably a real error. Threads through every cloned
    /// sub-scope.
    check_operands: bool,
    /// Known namespace prefixes — every `mod/` (the segment up to and including the
    /// last `/`) for which *some* `mod/<name>` global is loaded in the heap. Lets
    /// the unbound check stay silent on a qualified reference whose module we don't
    /// know (a module defined dynamically — `%load-string`, a required temp module —
    /// or in another file a single-file check didn't load): we can't prove such a
    /// name unbound. A typo in a *known* module (`mod/` present) is still flagged.
    /// `Arc` so per-scope `Ctx` clones don't copy the set. Populated by
    /// [`check_file`]; empty in fragment mode (so any qualified name is left alone).
    known_ns: Arc<HashSet<String>>,
    /// **KI-17 reachability set** — module prefixes this file makes reachable *itself*:
    /// every `(:use M)` in its header, every top-level `(require 'M)`, and its own
    /// namespace. `Some` only in whole-project mode ([`check_file`] with the image
    /// loaded), where an un-required module is nonetheless *bound* image-wide; a
    /// user-written qualified reference to a module outside this set then resolves only
    /// by load-order luck and is flagged. `None` disables the lint — in single-file /
    /// fragment / REPL mode an un-required module simply isn't bound, so the ordinary
    /// unbound check already covers it. `Arc` so per-scope clones don't copy the set.
    required_mods: Option<Arc<HashSet<String>>>,
    /// The full qualified symbol *names* (`"mod/name"`) that appear literally in the
    /// **un-expanded** source — the user-written references. The KI-17 lint fires only
    /// for a reference in this set, so a *macro-injected* `other/helper` (present only in
    /// the expanded tree, naming a module the user's file never mentions) is never
    /// flagged. `Arc` so per-scope clones stay cheap.
    raw_qualified: Arc<HashSet<String>>,
    /// Names the top-level form currently being walked guards with an explicit
    /// `(bound? 'name)` test. Such a reference is *correct code for an image that
    /// doesn't define the name* — the whole point of the guard — so the unbound
    /// diagnostic must not fire on it. See [`is_bound_guarded`](Ctx::is_bound_guarded).
    /// Reset per top-level form by `check_file`, so the exemption is scoped to the
    /// function that actually does the guarding. `Arc` so per-scope clones stay cheap.
    bound_guarded: Arc<HashSet<Symbol>>,
}

impl Ctx {
    /// The locally-known type for `sym`, or `None` if it isn't tracked.
    pub(super) fn get(&self, sym: Symbol) -> Option<Ty> {
        self.types.get(&sym).cloned()
    }
    /// The guard (variable + asserted type) `sym` was bound to, if any.
    pub(super) fn guard(&self, sym: Symbol) -> Option<(Symbol, Ty)> {
        self.guards.get(&sym).cloned()
    }
    /// Is `sym` in scope here? — a local binder (fn-param or let), a recorded
    /// narrowing, guard alias, or let-binding alias, or an accumulated
    /// file-global. Bindings in the surrounding heap (prelude, builtins,
    /// earlier-defined globals in a real runtime) are checked separately by
    /// the caller — this is the *local* view only.
    pub(super) fn is_local(&self, sym: Symbol) -> bool {
        self.locals.contains(&sym)
            || self.types.contains_key(&sym)
            || self.guards.contains_key(&sym)
            || self.aliases.contains_key(&sym)
            || self.file_globals.contains(&sym)
    }
    /// Is `sym` a `def`/`defn`/`defdyn`-defined **file-global** (as opposed to a
    /// lexical binder)? Used by the unused-`let` lint to tell a deliberate shadow
    /// of a file-global (`(let (*dt* 5) …)`) from a genuine leftover.
    pub(super) fn is_file_global(&self, sym: Symbol) -> bool {
        self.file_globals.contains(&sym)
    }
    /// A copy of this ctx with the given lint categories additionally suppressed
    /// (a `(check-allow …)` scope entered). ORs into any already-suppressed set.
    /// The file's ability facts, if any (`None` unless `set_ability` ran at file level).
    pub(super) fn ability(&self) -> Option<&super::protocol::AbilityInfo> {
        self.ability.as_deref()
    }

    /// Install the file's ability facts (once, at the top of file checking).
    pub(super) fn set_ability(&mut self, info: std::sync::Arc<super::protocol::AbilityInfo>) {
        self.ability = Some(info);
    }

    /// The file's multimethod facts, if any (`None` unless `set_multi` ran at file level).
    pub(super) fn multi(&self) -> Option<&super::protocol::MultiInfo> {
        self.multi.as_deref()
    }

    /// Install the file's multimethod facts (once, at the top of file checking).
    pub(super) fn set_multi(&mut self, info: std::sync::Arc<super::protocol::MultiInfo>) {
        self.multi = Some(info);
    }

    /// A clone marked as inferring `sym`'s signature — so a self-recursive call is skipped
    /// in return-union inference (see the `inferring_self` field).
    pub(super) fn with_inferring_self(&self, sym: Symbol) -> Ctx {
        let mut c = self.clone();
        c.inferring_self = Some(sym);
        c
    }
    /// The function whose signature is being inferred, if this `Ctx` is a return-inference one.
    pub(super) fn inferring_self(&self) -> Option<Symbol> {
        self.inferring_self
    }

    /// Whether strict mode is on for this check — see [`Ctx::strict`].
    pub(super) fn strict(&self) -> bool {
        self.strict_mode
    }

    /// Turn strict mode on or off (set once, at the root of a file check).
    pub(super) fn set_strict(&mut self, on: bool) {
        self.strict_mode = on;
    }

    pub(super) fn with_suppressed(&self, mask: u8) -> Ctx {
        let mut c = self.clone();
        c.suppressed |= mask;
        c
    }
    /// Is any category in `mask` currently suppressed by an enclosing
    /// `(check-allow …)`? Checked by a lint before emitting a warning.
    pub(super) fn is_suppressed(&self, mask: u8) -> bool {
        self.suppressed & mask != 0
    }
    /// Is `sym` a genuine *lexical* binder in scope — a fn/lambda/defn param or a
    /// `let`/`letrec` name (the `locals` set), as opposed to a guard-narrowed free
    /// variable or an accumulated file-global? A lexical local can never be a
    /// macro, so a call with such a head evaluates its arguments — which is what
    /// the operand-unbound check needs to know (`evaluates_args` in `walk`).
    pub(super) fn is_lexical_local(&self, sym: Symbol) -> bool {
        self.locals.contains(&sym)
    }
    /// **Narrow** `sym` to the intersection with `ty` (a guard refinement —
    /// the same lexical variable in the same scope getting tighter). The
    /// caller already knows `sym` lives in this scope (e.g. it's a free
    /// variable inside an `if`'s branch); for an unknown one we treat the
    /// prior as `ANY`, so the intersection is just `ty`.
    ///
    /// **Alias propagation.** If `sym` is an alias for another local (via
    /// `(let (sym other) …)`), narrowing `sym` also narrows `other`, and
    /// recursively any further alias chain. That's how a narrowing on
    /// `match`'s internal scrutinee `m__28` reaches the user-visible variable
    /// `x` the `let` bound it to.
    pub(super) fn narrow(&self, sym: Symbol, ty: Ty) -> Ctx {
        let mut c = self.clone();
        c.narrow_chain(sym, ty);
        c
    }
    /// Narrow the compound path `base.keys…` (a field/index access chain) to
    /// `prior ∩ ty` for the returned scope — occurrence typing under a guard.
    /// Keyed by `(base, keys)`; intersects with any prior narrowing so nested
    /// guards compose. Sound only because Brood values are immutable (see
    /// `path_types`).
    pub(super) fn narrow_path(&self, base: Symbol, keys: Vec<PathKey>, ty: Ty) -> Ctx {
        let mut c = self.clone();
        let prior = c
            .path_types
            .get(&(base, keys.clone()))
            .cloned()
            .unwrap_or(Ty::ANY);
        c.path_types.insert((base, keys), prior.intersect(ty));
        c
    }
    /// The narrowed type of the path `base.keys…`, if a guard asserted one.
    pub(super) fn path_ty(&self, base: Symbol, keys: &[PathKey]) -> Option<Ty> {
        self.path_types.get(&(base, keys.to_vec())).cloned()
    }
    /// In-place narrow over the equivalence class of `sym` — BFS through the
    /// alias graph, intersecting `ty` into each visited name's type. A
    /// `visited` set caps each name at one narrow so a cycle (the
    /// always-present bidirectional edge) terminates cleanly.
    fn narrow_chain(&mut self, sym: Symbol, ty: Ty) {
        let mut visited = HashSet::new();
        let mut queue = vec![sym];
        while let Some(s) = queue.pop() {
            if !visited.insert(s) {
                continue;
            }
            let prior = self.types.get(&s).cloned().unwrap_or(Ty::ANY);
            self.types.insert(s, prior.intersect(ty.clone()));
            if let Some(neighbours) = self.aliases.get(&s) {
                for &n in neighbours {
                    if !visited.contains(&n) {
                        queue.push(n);
                    }
                }
            }
        }
    }
    /// **Bind** `sym` to `ty`, overwriting any prior entry — a fresh let-bound
    /// or fn-param variable shadows the outer. `None` clears the type entry so
    /// a shadowing binding of unknown type doesn't keep an outer narrowing
    /// (but the name is still in scope via `locals`, so an unbound check
    /// doesn't fire on it). Disconnects `sym` from the alias graph entirely
    /// — removes its bin and also removes it from every neighbour's bin —
    /// so a fresh binding doesn't inherit aliases through stale back-edges.
    pub(super) fn bind(&self, sym: Symbol, ty: Option<Ty>) -> Ctx {
        let mut c = self.clone();
        match ty {
            Some(t) => {
                c.types.insert(sym, t);
            }
            None => {
                c.types.remove(&sym);
            }
        }
        c.locals.insert(sym);
        c.guards.remove(&sym);
        // A fresh binding of `sym` invalidates any `(get sym :k)` path narrowing —
        // the new value is unrelated to whatever a prior guard asserted.
        c.path_types.retain(|(base, _), _| *base != sym);
        // A fresh binding shadows the sig-typed param / dead-clause local of the
        // same name — the new binding's type is unrelated, so it must not drive
        // the dead-clause lint.
        c.sig_params.remove(&sym);
        c.dead_clause_locals.remove(&sym);
        if let Some(neighbours) = c.aliases.remove(&sym) {
            for n in neighbours {
                if let Some(set) = c.aliases.get_mut(&n) {
                    set.remove(&sym);
                }
            }
        }
        c
    }
    /// Record that `sym` was let-bound to the result of testing `target` for
    /// `ty` — so a later `(if sym then else)` narrows `target` accordingly.
    /// Self-aliasing (`(let (x (int? x)) …)` would shadow the outer `x` the
    /// guard means to narrow) is rejected.
    pub(super) fn add_guard(&self, sym: Symbol, target: Symbol, ty: Ty) -> Ctx {
        if sym == target {
            return self.clone();
        }
        let mut c = self.clone();
        c.guards.insert(sym, (target, ty));
        c
    }
    /// Record `(let (sym target) …)` — an undirected alias. Each side gets
    /// the other added to its neighbour-set, so a later `narrow` on either
    /// reaches both via `narrow_chain`'s BFS. Self-aliases are rejected
    /// (no-op): `(let (x x) …)` shadows the outer `x` and "aliasing itself"
    /// would just add a vacuous self-loop.
    pub(super) fn add_alias(&self, sym: Symbol, target: Symbol) -> Ctx {
        if sym == target {
            return self.clone();
        }
        let mut c = self.clone();
        c.aliases.entry(sym).or_default().insert(target);
        c.aliases.entry(target).or_default().insert(sym);
        c
    }
    /// Record a top-level `(def/defn/defmacro name …)` so subsequent forms in
    /// the same file see `name` as bound (the file isn't being evaluated, so
    /// `name` won't appear in `heap`'s global table). In-place mutation; the
    /// accumulator threads through [`check_file`].
    pub(super) fn add_file_global(&mut self, sym: Symbol) {
        self.file_globals.insert(sym);
    }
    /// Record a file-local `(defmacro name …)` — both as a file-global (it's a
    /// bound name) and in the macro set (its calls take opaque syntax).
    pub(super) fn add_file_macro(&mut self, sym: Symbol) {
        self.file_globals.insert(sym);
        self.file_macros.insert(sym);
    }
    /// Is `sym` a file-local macro name accumulated by [`check_file`]?
    pub(super) fn is_file_macro(&self, sym: Symbol) -> bool {
        self.file_macros.contains(&sym)
    }
    /// Adopt the heap's cached, shared prefix set ([`Heap::known_ns_prefixes`]) as `known_ns`
    /// (each prefix ends in `/`) — an O(1) `Arc` bump, so a whole-project check builds it once
    /// instead of per file.
    pub(super) fn set_known_ns_arc(&mut self, prefixes: Arc<HashSet<String>>) {
        self.known_ns = prefixes;
    }
    /// Is `prefix` (a `mod/` segment, trailing slash included) a namespace the
    /// loaded image knows? Used to decide whether an unresolved *qualified* name is
    /// a real unbound reference or a dynamically/elsewhere-defined one.
    pub(super) fn module_is_known(&self, prefix: &str) -> bool {
        self.known_ns.contains(prefix)
    }
    /// Record the KI-17 reachability set (see [`required_mods`](Ctx::required_mods)) —
    /// enables the unrequired-module lint for this (whole-file) check.
    pub(super) fn set_required_mods(&mut self, mods: HashSet<String>) {
        self.required_mods = Some(Arc::new(mods));
    }
    /// The file's reachability set, or `None` when the lint is disabled (fragment mode).
    pub(super) fn required_mods(&self) -> Option<&HashSet<String>> {
        self.required_mods.as_deref()
    }
    /// Record the set of user-written qualified symbol names (see
    /// [`raw_qualified`](Ctx::raw_qualified)).
    pub(super) fn set_raw_qualified(&mut self, names: HashSet<String>) {
        self.raw_qualified = Arc::new(names);
    }
    /// Did the qualified name `name` (`"mod/name"`) appear literally in the source?
    pub(super) fn raw_qualified_has(&self, name: &str) -> bool {
        self.raw_qualified.contains(name)
    }
    /// Record the `(bound? 'name)`-guarded names of the top-level form about to be
    /// walked (see [`bound_guarded`](Ctx::bound_guarded)).
    pub(super) fn set_bound_guarded(&mut self, names: HashSet<Symbol>) {
        self.bound_guarded = Arc::new(names);
    }
    /// Does the enclosing top-level form test `sym` with `(bound? 'sym)`? Then a
    /// reference to it is deliberately conditional — an ambient global some other
    /// module `def`s (`*project-name*`, set at project setup) — and reporting it
    /// unbound would flag code that is correct precisely *because* of the guard.
    pub(super) fn is_bound_guarded(&self, sym: Symbol) -> bool {
        self.bound_guarded.contains(&sym)
    }
    /// Record that file-local `sym`'s value is a **variadic** `fn` (has a `&`
    /// rest param). Consulted by the arity check so a `(sig …)`-derived *exact*
    /// arity is never used to flag a variadic defn (see `variadic_globals`).
    pub(super) fn mark_variadic_global(&mut self, sym: Symbol) {
        self.variadic_globals.insert(sym);
    }
    /// The inferred per-arm signatures of a same-file multi-arm function, if any.
    pub(super) fn inferred_overload(&self, sym: Symbol) -> Option<Vec<Sig>> {
        self.inferred_overload.get(&sym).cloned()
    }
    /// Record a same-file multi-arm function's per-arm signatures (Pass 2.8).
    pub(super) fn add_inferred_overload(&mut self, sym: Symbol, sigs: Vec<Sig>) {
        self.inferred_overload.insert(sym, sigs);
    }
    /// Is `sym` a file-local definition whose value is a variadic `fn`?
    pub(super) fn is_variadic_global(&self, sym: Symbol) -> bool {
        self.variadic_globals.contains(&sym)
    }
    /// Record the arity a same-file definition of `sym` admits. A name defined more
    /// than once (a redefinition, or a `def` in two branches) merges to the interval
    /// **hull** of the two — accepting a call either definition would accept, which is
    /// the only sound reading when the checker can't say which one a given call sees.
    pub(super) fn add_file_arity(&mut self, sym: Symbol, arity: Arity) {
        self.file_arity
            .entry(sym)
            .and_modify(|a| {
                *a = Arity {
                    min: a.min.min(arity.min),
                    max: match (a.max, arity.max) {
                        (Some(x), Some(y)) => Some(x.max(y)),
                        _ => None,
                    },
                };
            })
            .or_insert(arity);
    }
    /// The arity a same-file definition of `sym` admits, if the checker could read it.
    pub(super) fn file_arity(&self, sym: Symbol) -> Option<Arity> {
        self.file_arity.get(&sym).copied()
    }
    /// The user-declared signature for `sym` from a `(sig …)` form, if any.
    pub(super) fn declared_sig(&self, sym: Symbol) -> Option<Sig> {
        self.declared.get(&sym).cloned()
    }
    /// Record a `(sig name (… -> …))` declaration. In-place; threads through
    /// [`check_file`] like [`add_file_global`](Ctx::add_file_global).
    pub(super) fn add_declared_sig(&mut self, sym: Symbol, sig: Sig) {
        self.declared.insert(sym, sig);
    }
    /// The declared **value** type for `sym` from a non-arrow `(sig x T)`, if any.
    pub(super) fn declared_value_ty(&self, sym: Symbol) -> Option<Ty> {
        self.declared_value_ty.get(&sym).cloned()
    }
    /// Record a `(sig x T)` value-type declaration (`T` non-arrow).
    pub(super) fn add_declared_value_ty(&mut self, sym: Symbol, ty: Ty) {
        self.declared_value_ty.insert(sym, ty);
    }
    /// The **inferred** value type for undeclared global `sym` (Gap A), if one was
    /// recorded. Never returned when a declared value type exists (callers check
    /// [`declared_value_ty`] first). Callers must treat it as `dynamic_within`.
    pub(super) fn inferred_value_ty(&self, sym: Symbol) -> Option<Ty> {
        self.inferred_value_ty.get(&sym).cloned()
    }
    /// Record an inferred current-image value type for an undeclared,
    /// defined-exactly-once global (Gap A). No-op if a declared value type already
    /// exists (that's authoritative).
    pub(super) fn add_inferred_value_ty(&mut self, sym: Symbol, ty: Ty) {
        if !self.declared_value_ty.contains_key(&sym) {
            self.inferred_value_ty.insert(sym, ty);
        }
    }
    /// The same-file inferred function signature for `sym`, if one was recorded. Read
    /// *after* [`declared_sig`] (authoritative). Callers treat its return as an
    /// over-approximation (a call result), like a loaded-inferred sig.
    pub(super) fn inferred_fn_sig(&self, sym: Symbol) -> Option<Sig> {
        self.inferred_fn_sig.get(&sym).cloned()
    }
    /// Record a same-file inferred function sig (Pass 2.8). No-op if a sig is already
    /// declared for `sym` — a declaration wins.
    pub(super) fn add_inferred_fn_sig(&mut self, sym: Symbol, sig: Sig) {
        if !self.declared.contains_key(&sym) {
            self.inferred_fn_sig.insert(sym, sig);
        }
    }
    /// The full (variable-bearing) declared sig for `sym`, if it was parsed
    /// with at least one type variable.
    pub(super) fn declared_sig_with_vars(&self, sym: Symbol) -> Option<&SigWithVars> {
        self.declared_vars.get(&sym)
    }
    /// Record the type-variable-bearing sig alongside the flattened one.
    pub(super) fn add_declared_sig_with_vars(&mut self, sym: Symbol, sig: SigWithVars) {
        self.declared_vars.insert(sym, sig);
    }
    /// The declared overload (2+ distinct arrow sigs) for `sym`, if any.
    pub(super) fn declared_overload(&self, sym: Symbol) -> Option<&Vec<Sig>> {
        self.declared_overloads.get(&sym)
    }
    /// Record a `(sig name (and (A -> B) (C -> D) …))` overload declaration.
    pub(super) fn add_declared_overload(&mut self, sym: Symbol, sigs: Vec<Sig>) {
        self.declared_overloads.insert(sym, sigs);
    }
    /// Seed parameter `sym` with the type `ty` its enclosing function's `(sig …)`
    /// declared for it, and remember it as a sig-typed param (so a guard that
    /// later narrows it to `never` is a provable dead clause). Returns the
    /// extended scope.
    pub(super) fn bind_sig_param(&self, sym: Symbol, ty: Ty) -> Ctx {
        let mut c = self.bind(sym, Some(ty));
        c.sig_params.insert(sym);
        c
    }
    /// Is `sym` a parameter seeded from a `(sig …)` declaration? Its tracked type
    /// is then the *exact* contract type (or a guard-narrowed subset), so the
    /// gradual checks can treat it as `stat` (precise, `⊆`) rather than an
    /// over-approximated `dynamic`.
    pub(super) fn is_sig_param(&self, sym: Symbol) -> bool {
        self.sig_params.contains(&sym)
    }
    /// Mark an already-bound `let`-local as **dead-clause eligible** — see
    /// [`dead_clause_locals`](Ctx::dead_clause_locals). Called by `check_let` after
    /// `bind`, only for a surface, precisely-typed binding.
    pub(super) fn mark_dead_clause_local(&mut self, sym: Symbol) {
        self.dead_clause_locals.insert(sym);
    }
    /// After a guard narrowed this scope from `before`, return a **dead-clause-
    /// eligible binding that has just become the empty type** (with the type it had
    /// in `before`), if any — i.e. a sig-typed param or a precise surface `let`-local
    /// whose type is disjoint from what the guard asserts, so the branch is
    /// unreachable. Both sets are tiny (one scope's typed bindings), so the scan is
    /// cheap. Restricting to these two sets is what keeps the dead-clause lint sound
    /// and free of false positives on generated / redefinable bindings.
    /// Is this scope UNREACHABLE — has a guard narrowed some local to `never`? A branch
    /// entered under such a scope cannot run (the test that led here is contradicted by
    /// what is known of the value), so it is neither checked nor typed. What made this
    /// necessary: once `%vector-ref` on a literal tuple types as the exact element, the
    /// `with`/`match` lowering's `(%eq el :ok)` over a literal `[:error :nope]` has a then-
    /// branch whose `el` is `:error ∩ :ok = never` — a branch the runtime never enters,
    /// which the checker walked and reported `(+ a b)` in, with `b` the literal `:nope`.
    pub(super) fn is_dead(&self) -> bool {
        self.types.values().any(Ty::is_never)
    }

    pub(super) fn newly_dead_binding(&self, before: &Ctx) -> Option<(Symbol, Ty)> {
        self.sig_params
            .iter()
            .chain(self.dead_clause_locals.iter())
            .find_map(|&p| {
                let now_never = self.types.get(&p).is_some_and(Ty::is_never);
                let was_never = before.types.get(&p).is_some_and(Ty::is_never);
                if now_never && !was_never {
                    before.types.get(&p).map(|prior| (p, prior.clone()))
                } else {
                    None
                }
            })
    }
    /// Turn on operand / value-slot unbound checking — see [`check_operands`].
    /// [`check_file`] calls this on the root ctx so the whole-file walk runs
    /// strict; the flag rides every cloned sub-scope.
    pub(super) fn enable_operand_checks(&mut self) {
        self.check_operands = true;
    }
    /// Whether operand / value-slot unbound checking is on for this scope.
    pub(super) fn checks_operands(&self) -> bool {
        self.check_operands
    }
}
