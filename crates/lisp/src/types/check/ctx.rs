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

use crate::core::value::Symbol;
use crate::types::{Sig, Ty};

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
#[derive(Clone, Default)]
pub(super) struct Ctx {
    types: HashMap<Symbol, Ty>,
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
    /// `(sig name (… -> …))` declarations — authoritative signatures the user
    /// wrote, read *first* by the call-checker (ahead of primitive/curated/
    /// inferred). Populated by [`check_file`]'s scan of the un-expanded forms.
    /// Slice 1 trusts these without runtime enforcement; slice 2 (the strong
    /// arrow) makes that trust sound. See `docs/type-annotations.md`.
    declared: HashMap<Symbol, Sig>,
    /// Parameters whose type was **seeded from the enclosing function's `(sig …)`
    /// declaration** — the subset of `types` we trust enough to flag a *dead
    /// clause* on. A guard that narrows one of these to the empty type means a
    /// `match`/`cond` clause can never run (the declared type is incompatible
    /// with the pattern). Gating on this set is what keeps the dead-clause lint
    /// free of false positives: a literal scrutinee or a compiler-generated guard
    /// (destructure / `match` lowering) never involves a sig-typed param, so it
    /// is never flagged. Shadowing removes a name (see [`bind`](Ctx::bind)).
    sig_params: HashSet<Symbol>,
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
        // A fresh binding shadows the sig-typed param of the same name — the new
        // binding's type is unrelated, so it must not drive the dead-clause lint.
        c.sig_params.remove(&sym);
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
    /// The user-declared signature for `sym` from a `(sig …)` form, if any.
    pub(super) fn declared_sig(&self, sym: Symbol) -> Option<Sig> {
        self.declared.get(&sym).cloned()
    }
    /// Record a `(sig name (… -> …))` declaration. In-place; threads through
    /// [`check_file`] like [`add_file_global`](Ctx::add_file_global).
    pub(super) fn add_declared_sig(&mut self, sym: Symbol, sig: Sig) {
        self.declared.insert(sym, sig);
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
    /// After a guard narrowed this scope from `before`, return a **sig-typed
    /// param that has just become the empty type** (with the type it had in
    /// `before`), if any — i.e. a parameter whose declared type is disjoint from
    /// what the guard asserts, so the branch is unreachable. `sig_params` is tiny
    /// (one function's params), so this scan is cheap. Only sig-typed params are
    /// considered, which is exactly what makes the dead-clause lint sound.
    pub(super) fn newly_dead_sig_param(&self, before: &Ctx) -> Option<(Symbol, Ty)> {
        self.sig_params.iter().find_map(|&p| {
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
