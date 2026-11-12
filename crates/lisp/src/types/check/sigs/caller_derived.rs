//! The caller-derived parameter pass (ADR-341): parameter types for module-private
//! functions, derived from every call site in the file.

use super::*;

// ---- the caller-derived parameter pass: caller-derived parameter types for module-private functions (ADR-341) ----

/// Bound on the rounds of [`caller_derived_params`]: a chain of private helpers settles in
/// one round per level, and `json`'s parser is ten levels deep (`decode` → `json-value` →
/// `json-object` → `object-acc` → `json-string` → `string-acc` → `json-escape` →
/// `json-unicode` → `hex4` → `hex1`); a round is one walk of the file's forms.
pub(in crate::types::check) const MAX_DERIVE_ROUNDS: usize = 32;

/// The round of a derivation from which every moving type is widened (see
/// [`Ty::widened_below`]) — after the early rounds a deep chain needs to settle exactly.
pub(in crate::types::check) const WIDEN_AFTER_ROUND: usize = 12;
/// The depth a widened type keeps: a record's fields and a sequence's elements, and those
/// elements' own structure one level down — `vector<vector<int>>` survives, a JSON value's
/// third level of nesting does not.
pub(in crate::types::check) const WIDEN_DEPTH: usize = 2;

/// The **caller-derived parameter types** of this file's module-private functions: for each
/// candidate in `candidates` (name → its single-arm `(fn …)` form), per parameter, the
/// union of the types every call site in `forms` hands it, as the least fixpoint over the
/// file — a site inside another private function is typed with THAT function's derived
/// parameters bound, so `int` flows from the public entry down through every helper.
///
/// **Why this is sound.** A `defn-` is callable only from its own file, so its callers are
/// exactly the call sites here; every site's argument is typed by `expr_ty` under a scope
/// whose bindings over-approximate the values (a public function's parameters are unknown,
/// a private one's are its derived types, a `let` binder is its RHS's type), and the
/// fixpoint closes over the sites inside private bodies. By induction along any chain of
/// activations from a public entry, every actual argument lies in the derived type.
/// Everything that could break the closed-caller premise declines the function instead:
/// the name used as a VALUE anywhere (`(map xs helper)`, `(apply helper …)`, inside quoted
/// data) — `escapes`; an unexpanded macro call anywhere in the file (its operands are
/// syntax that may construct a call the walk cannot see) — the whole pass declines; a call
/// of the wrong arity (it raises, contributing nothing); a function with no site at all
/// (dead code — unknown, not `never`, so its body is still checked as written). A position
/// some site cannot type is unknown. Not reaching a fixpoint within the bound declines too.
///
/// `(:use-internals mod)` lets another file call a private function; the derived types are
/// used only for THIS file's walk and this file's same-file return inference, both of which
/// describe this file's own calls, and a loaded-closure caller elsewhere reads the
/// demand-based inference as before.
pub(in crate::types::check) fn caller_derived_params(
    heap: &Heap,
    forms: &[Value],
    candidates: &HashMap<Symbol, Value>,
    live: &HashMap<Symbol, usize>,
    ctx: &Ctx,
    resume_from: &HashMap<Symbol, Vec<Option<Ty>>>,
    cache: &mut SiteCache,
    moved: Option<&HashSet<Symbol>>,
) -> HashMap<Symbol, Vec<Option<Ty>>> {
    let targets: HashSet<Symbol> = candidates.keys().copied().collect();
    // A LEAST fixpoint starts at ⊥: the live candidates — those with a site and no
    // escape (`live`, name → arity, found by [`live_private_functions`] ONCE per file since
    // neither depends on any type)
    // — begin with every parameter `never`, so a self-call's `(+ i 1)` contributes nothing
    // until some other caller has said what `i` is. Started at UNKNOWN instead, that same
    // argument read `number` in the first round and nothing could ever narrow it back.
    // An escaped or site-less candidate stays out of the map, hence unknown: its callers
    // are not all here, so nothing may be assumed of the arguments its body hands on.
    // …or from `resume_from`, the previous joint round's result (the caller-derived parameter pass alternates this
    // with the returns): computed under LOWER returns, it lies below the fixpoint sought
    // now, so continuing from it is the same ascent with the early rounds already done.
    let mut derived: HashMap<Symbol, Vec<Option<Ty>>> = live
        .iter()
        .map(|(&name, &arity)| {
            let seed = resume_from
                .get(&name)
                .cloned()
                .unwrap_or_else(|| vec![Some(Ty::NEVER); arity]);
            (name, seed)
        })
        .collect();
    let mut older: HashMap<Symbol, Vec<Option<Ty>>> = HashMap::new();
    // What moved since the cache's last walk: the caller's answer for the first round
    // (the returns and derived parameters the joint round changed), then the parameters
    // this round moved — within these rounds the context is fixed, so a form's walk can
    // change only through its own parameters (`SiteCache`).
    let mut moved: Option<HashSet<Symbol>> = moved.cloned();
    for round in 0..MAX_DERIVE_ROUNDS {
        let Some(collected) = collect_private_sites(
            heap,
            forms,
            &targets,
            candidates,
            &derived,
            ctx,
            cache,
            moved.as_ref(),
        ) else {
            DERIVATION_GAVE_UP.with(|c| c.set(Some(DeriveDecline::Opaque)));
            return HashMap::new();
        };
        let mut next: HashMap<Symbol, Vec<Option<Ty>>> = HashMap::new();
        // A fixed order: typing a site spends the file's specialization fuel and fills
        // its memo, so hash order would make a round's answers depend on the process.
        let mut names: Vec<Symbol> = collected.names().into_iter().collect();
        names.sort_by_key(|name| value::symbol_name_ref(*name));
        for name in names {
            let sites = collected.sites_of(name);
            // Only a LIVE candidate is derived: one whose only sites are its own recursion
            // (or that escapes, or has none) is not in `live` and must stay unknown. Derived
            // from its self-call alone, it took whatever the call's branch narrowed its
            // argument to — `(if y … (f y))` typed `y` nil|false — and the branch beside the
            // call went dead, unchecked: every warning in it was lost.
            if !live.contains_key(&name) || collected.escaped(name) || sites.is_empty() {
                continue;
            }
            let arity = sites[0].arity();
            let mut acc: Vec<Option<Ty>> = vec![Some(Ty::NEVER); arity];
            for site in &sites {
                for k in 0..arity {
                    if let Some(current) = acc[k].clone() {
                        acc[k] = site.param_ty(heap, k, ctx).map(|t| current.union(t));
                    }
                }
            }
            // A parameter whose last value appears INSIDE this round's — a value type
            // nesting one level deeper each round — is folded into a RECURSIVE type
            // (`Ty::fold_recursive`, ADR-349): the candidate `μX. G[X]`, which the next
            // round confirms by folding back to it, or moves past. WIDENING
            // (`Ty::widened_below`) remains for what neither converges nor folds after
            // the early rounds: cut to a fixed depth, after which the ascent is
            // stationary. The early rounds are left exact so a chain of helpers settles
            // at full precision first.
            // The value a round nests may be the one from TWO rounds back — a value
            // that recurses through another function's parameter grows by a level every
            // other round — so both are tried, the nearer first.
            for (k, t) in acc.iter_mut().enumerate() {
                let Some(t) = t else { continue };
                let recent = [derived.get(&name), older.get(&name)]
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.get(k).cloned().flatten());
                // An INTERVAL that moved since the last round goes to its infinity
                // (`Ty::widen_intervals_against`, ADR-350): a counter's `[0,0], [0,1], …`
                // is `[0, ∞)` in one step, where the depth widening never saw it move.
                // Before the fold, so a moving length does not hide a nesting.
                if let Some(Some(prev)) = derived.get(&name).and_then(|p| p.get(k)) {
                    *t = t.widen_intervals_against(prev);
                }
                for prev in recent {
                    if let Some(folded) = Ty::fold_recursive(&prev, t) {
                        *t = folded;
                        break;
                    }
                }
                if round >= WIDEN_AFTER_ROUND {
                    *t = t.widened_below(WIDEN_DEPTH);
                }
            }
            next.insert(name, acc);
        }
        if next == derived {
            return derived;
        }
        let changed: HashSet<Symbol> = next
            .keys()
            .chain(derived.keys())
            .copied()
            .filter(|name| next.get(name) != derived.get(name))
            .collect();
        if crate::diagnostics::debug_flags::derive_debug() {
            for name in &changed {
                let shown: Vec<String> = next
                    .get(name)
                    .map(|tys| {
                        tys.iter()
                            .map(|t| t.as_ref().map_or("?".into(), |t| t.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                eprintln!(
                    "[derive] round {round} {} -> {shown:?}",
                    value::symbol_name_ref(*name)
                );
            }
        }
        moved = Some(changed);
        older = std::mem::replace(&mut derived, next);
    }
    if crate::diagnostics::debug_flags::derive_debug() {
        eprintln!("[derive] NO FIXPOINT");
    }
    DERIVATION_GAVE_UP.with(|c| c.set(Some(DeriveDecline::NoFixpoint)));
    HashMap::new()
}

/// Why the last [`caller_derived_params`] declined, if it did — read and cleared by the
/// joint loop, which reports it (B6): a derivation that gives up leaves every parameter
/// unknown, which reads as "zero warnings" unless something says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::types::check) enum DeriveDecline {
    /// The file holds an unexpanded macro call, which could construct a call the walk
    /// cannot see.
    Opaque,
    /// The parameters did not settle within [`MAX_DERIVE_ROUNDS`].
    NoFixpoint,
}

thread_local! {
    static DERIVATION_GAVE_UP: Cell<Option<DeriveDecline>> = const { Cell::new(None) };
}

/// Take the last decline (see [`DeriveDecline`]), leaving none.
pub(in crate::types::check) fn take_derivation_decline() -> Option<DeriveDecline> {
    DERIVATION_GAVE_UP.with(|c| c.take())
}

/// One place a candidate is called from: a direct call with its arguments and the scope
/// they are typed in, or a HANDOVER to a combinator that promises what it calls the
/// candidate with — `(map xs helper)` calls `helper` with `xs`'s elements, `(fold xs init
/// helper)` with the fold's accumulator and an element, a callee with a declared arrow with
/// that arrow's parameters (`walk::callback_seed`, the same three promises the walk seeds a
/// `fn` literal's parameters from). Anywhere else a bare name goes is an escape.
enum Site {
    /// The arguments, the scope they are typed in, and the candidate whose own body the
    /// call sits in (a self-call), if any.
    Call(Vec<Value>, Box<Ctx>, Option<Symbol>),
    Handover(Vec<Ty>),
}

impl Site {
    fn arity(&self) -> usize {
        match self {
            Site::Call(args, _, _) => args.len(),
            Site::Handover(params) => params.len(),
        }
    }
    /// Does this site hand parameter `n` the count of what it hands parameter `xs`? The
    /// `xs` argument must be a plain symbol `b`, and the `n` argument `(count b)` (or its
    /// string/vector spelling, unshadowed) or a symbol the site's scope knows as `b`'s
    /// count alias. A handover promises no such relation.
    fn counts(&self, heap: &Heap, n: usize, xs: usize) -> bool {
        let Site::Call(args, scope, _) = self else {
            return false;
        };
        let (Some(&Value::Sym(b)), Some(&n_arg)) = (args.get(xs), args.get(n)) else {
            return false;
        };
        match n_arg {
            Value::Sym(a) => scope.count_alias(a) == Some(b),
            _ => match list_items(heap, n_arg).as_deref() {
                Some([Value::Sym(head), Value::Sym(arg)]) => {
                    *arg == b
                        && !scope.is_lexical_local(*head)
                        && (value::symbol_is(*head, "count")
                            || value::symbol_is(*head, "%string-length"))
                }
                _ => false,
            },
        }
    }
    /// What this site hands parameter `k`: an argument's type in its scope, or the
    /// combinator's promise. The scope was captured when the site was walked, which may
    /// be an earlier round (`SiteCache`): its file-level facts — the returns and derived
    /// parameters every round moves — are re-based on the CURRENT context before the
    /// argument is typed, so a cached site never reads a stale return.
    fn param_ty(&self, heap: &Heap, k: usize, ctx: &Ctx) -> Option<Ty> {
        match self {
            Site::Call(args, scope, _) => {
                let scope = scope.with_file_of(ctx);
                super::infer::with_fresh_depth(|| expr_ty(heap, args[k], &scope))
            }
            Site::Handover(params) => Some(params[k].clone()),
        }
    }
}

/// One top-level form's sites and escapes.
#[derive(Default)]
struct PrivateSites {
    /// Per candidate, every site of the right arity.
    sites: HashMap<Symbol, Vec<Site>>,
    /// Candidates whose name occurs somewhere other than a call head or a handover.
    escaped: HashSet<Symbol>,
}

/// Every form's sites, read across the parts without merging them (a part is shared with
/// the cache that may hand it out again next round).
struct CollectedSites {
    parts: Vec<Rc<PrivateSites>>,
}

impl CollectedSites {
    fn names(&self) -> HashSet<Symbol> {
        self.parts
            .iter()
            .flat_map(|part| part.sites.keys().copied())
            .collect()
    }
    fn escaped(&self, name: Symbol) -> bool {
        self.parts.iter().any(|part| part.escaped.contains(&name))
    }
    fn sites_of(&self, name: Symbol) -> Vec<&Site> {
        self.parts
            .iter()
            .filter_map(|part| part.sites.get(&name))
            .flat_map(|sites| sites.iter())
            .collect()
    }
}

/// The site walk, memoised per top-level form across the rounds of one file's derivation.
/// The walk is the derivation's cost — it re-types every `let` binding and
/// every guard in the file (`let_bind_scope`, `branch_scopes`) to build the scope each site
/// is typed in — and it ran whole on every round of every joint round: 4.2 s of the 11 s
/// `nest check` spent over `std/`, against 0.2 s typing the sites it collected. A form's
/// walk depends on exactly two moving things: the derived types of its OWN parameters
/// (bound by the `fn` arm) and, through the scope, the returns and derived parameters of
/// the candidates its body reaches — transitively, since typing a call re-types the
/// callee's body under the arguments (`specialize_call`). So a form is re-walked when
/// either moved, and its last walk is handed out otherwise; the sites' captured scopes are
/// re-based on the current facts when read (`Site::param_ty`). `BROOD_NO_DERIVE_CACHE=1`
/// walks everything every round, and `check_derivation_cache_is_inert` runs both over the
/// tree and requires the same verdicts.
pub(in crate::types::check) struct SiteCache {
    /// Per form: the own-parameter types the last walk was under, and its result.
    walked: Vec<Option<(Option<Vec<Option<Ty>>>, Rc<PrivateSites>)>>,
    /// Per form: the candidate a top-level `(def name …)` binds.
    def_of: Vec<Option<Symbol>>,
    /// Per form: the candidates it reaches, transitively through the candidates' bodies
    /// (always including the form's own definition).
    reaches: Vec<HashSet<Symbol>>,
    /// The file holds an unexpanded macro call: every walk declines.
    opaque: bool,
    disabled: bool,
    /// Walks done and walks saved, for `BROOD_DERIVE_DBG`.
    pub(in crate::types::check) walks: u32,
    pub(in crate::types::check) reuses: u32,
}

impl SiteCache {
    pub(in crate::types::check) fn new(
        heap: &Heap,
        forms: &[Value],
        candidates: &HashMap<Symbol, Value>,
    ) -> SiteCache {
        let def_of: Vec<Option<Symbol>> = forms
            .iter()
            .map(|&form| {
                let items = list_items(heap, form)?;
                match (items.first(), items.get(1)) {
                    (Some(&Value::Sym(head)), Some(&Value::Sym(name)))
                        if value::symbol_is(head, kw::DEF) && candidates.contains_key(&name) =>
                    {
                        Some(name)
                    }
                    _ => None,
                }
            })
            .collect();
        // The candidates each form names, by spelling — every symbol in it, quoted or not.
        let refs: Vec<HashSet<Symbol>> = forms
            .iter()
            .map(|&form| {
                let mut out = HashSet::new();
                let mut work = vec![form];
                while let Some(v) = work.pop() {
                    match v {
                        Value::Sym(s) => {
                            if candidates.contains_key(&s) {
                                out.insert(s);
                            }
                        }
                        Value::Pair(_) => {
                            if let Some(items) = list_items(heap, v) {
                                work.extend(items);
                            }
                        }
                        Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
                        Value::Map(id) => {
                            for (k, val) in heap.map_entries(id) {
                                work.push(k);
                                work.push(val);
                            }
                        }
                        _ => {}
                    }
                }
                out
            })
            .collect();
        let refs_of: HashMap<Symbol, &HashSet<Symbol>> = def_of
            .iter()
            .zip(&refs)
            .filter_map(|(name, set)| name.map(|n| (n, set)))
            .collect();
        let reaches: Vec<HashSet<Symbol>> = refs
            .iter()
            .zip(&def_of)
            .map(|(direct, own)| {
                let mut closure: HashSet<Symbol> = direct.clone();
                closure.extend(*own);
                let mut work: Vec<Symbol> = closure.iter().copied().collect();
                while let Some(name) = work.pop() {
                    if let Some(more) = refs_of.get(&name) {
                        for &m in more.iter() {
                            if closure.insert(m) {
                                work.push(m);
                            }
                        }
                    }
                }
                closure
            })
            .collect();
        SiteCache {
            walked: vec![None; forms.len()],
            def_of,
            reaches,
            opaque: false,
            disabled: std::env::var_os("BROOD_NO_DERIVE_CACHE").is_some(),
            walks: 0,
            reuses: 0,
        }
    }
}

/// The call sites of every candidate across the file's expanded top-level forms, each with
/// its enclosing scope (see [`self_call_sites`] for the scope rules) — the parameters of a
/// candidate's own body bound to its CURRENT derived types, a public function's unknown.
/// `None` when the file holds an unexpanded macro call, which could construct a call the
/// walk cannot see. `moved` names the candidates whose returns or derived parameters
/// changed since the cache's last walk — `None` when unknown, which walks every form.
fn collect_private_sites(
    heap: &Heap,
    forms: &[Value],
    targets: &HashSet<Symbol>,
    candidates: &HashMap<Symbol, Value>,
    derived: &HashMap<Symbol, Vec<Option<Ty>>>,
    ctx: &Ctx,
    cache: &mut SiteCache,
    moved: Option<&HashSet<Symbol>>,
) -> Option<CollectedSites> {
    if cache.opaque {
        return None;
    }
    let mut parts: Vec<Rc<PrivateSites>> = Vec::with_capacity(forms.len());
    for (index, &form) in forms.iter().enumerate() {
        let own: Option<Vec<Option<Ty>>> =
            cache.def_of[index].and_then(|n| derived.get(&n).cloned());
        let reusable = !cache.disabled
            && moved.is_some_and(|moved| moved.is_disjoint(&cache.reaches[index]))
            && cache.walked[index]
                .as_ref()
                .is_some_and(|(walked_under, _)| *walked_under == own);
        if reusable {
            cache.reuses += 1;
            parts.push(
                cache.walked[index]
                    .as_ref()
                    .map(|(_, part)| part.clone())
                    .expect("checked"),
            );
            continue;
        }
        let Some(part) = collect_form_sites(heap, form, targets, candidates, derived, ctx) else {
            cache.opaque = true;
            return None;
        };
        let part = Rc::new(part);
        cache.walks += 1;
        cache.walked[index] = Some((own, part.clone()));
        parts.push(part);
    }
    Some(CollectedSites { parts })
}

/// One top-level form's walk — see [`collect_private_sites`].
fn collect_form_sites(
    heap: &Heap,
    form: Value,
    targets: &HashSet<Symbol>,
    candidates: &HashMap<Symbol, Value>,
    derived: &HashMap<Symbol, Vec<Option<Ty>>>,
    ctx: &Ctx,
) -> Option<PrivateSites> {
    struct Walker<'a> {
        heap: &'a Heap,
        targets: &'a HashSet<Symbol>,
        candidates: &'a HashMap<Symbol, Value>,
        derived: &'a HashMap<Symbol, Vec<Option<Ty>>>,
        ctx: &'a Ctx,
        out: PrivateSites,
        opaque: bool,
        /// The candidate whose single-arm body is being walked — a site found here is a
        /// self-call of it.
        within: Option<Symbol>,
        /// The parameter types of the `fn` LITERAL about to be walked — a `let`-bound
        /// literal's caller-derived types (ADR-355, set by the `let` arm) or a callback
        /// literal's promised types (set by `walk_args` from `callback_seed`) — left for
        /// exactly the next form and taken by the `fn` arm, so the literal's body is walked
        /// under what the walk binds it to.
        pending_fn_params: Option<Vec<Option<Ty>>>,
    }
    impl Walker<'_> {
        /// Quoted data is not namespace-resolved, so a target appears there under its BARE
        /// spelling (`'(bump)` for `t/bump`); either spelling escapes it.
        fn escape_in_data(&mut self, form: Value) {
            let mut work = vec![form];
            while let Some(v) = work.pop() {
                match v {
                    Value::Sym(s) if self.targets.contains(&s) => {
                        self.out.escaped.insert(s);
                    }
                    Value::Sym(s) => {
                        let bare = value::symbol_name_ref(s);
                        let hits: Vec<Symbol> = self
                            .targets
                            .iter()
                            .copied()
                            .filter(|t| {
                                let name = value::symbol_name_ref(*t);
                                name.rsplit('/').next() == Some(bare)
                            })
                            .collect();
                        self.out.escaped.extend(hits);
                    }
                    Value::Pair(_) => {
                        if let Some(items) = list_items(self.heap, v) {
                            work.extend(items);
                        }
                    }
                    Value::Vector(id) => work.extend(self.heap.vector(id).iter().copied()),
                    Value::Map(id) => {
                        for (k, val) in self.heap.map_entries(id) {
                            work.push(k);
                            work.push(val);
                        }
                    }
                    _ => {}
                }
            }
        }
        /// Walk `form` in `scope`; `def_of` is the candidate a `(def name …)` value slot
        /// belongs to, so its `(fn …)` binds the derived parameters. Grows the stack as the
        /// other body walkers do: this runs over EVERY file, and a macro can construct a
        /// body deeper than the reader would ever read.
        fn walk(&mut self, form: Value, scope: &Ctx, def_of: Option<Symbol>) {
            crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
                self.walk_inner(form, scope, def_of)
            })
        }
        fn walk_inner(&mut self, form: Value, scope: &Ctx, def_of: Option<Symbol>) {
            let heap = self.heap;
            match form {
                Value::Sym(s) if self.targets.contains(&s) => {
                    self.out.escaped.insert(s);
                    return;
                }
                Value::Pair(_) => {}
                Value::Vector(id) => {
                    for it in heap.vector(id).to_vec() {
                        self.walk(it, scope, None);
                    }
                    return;
                }
                Value::Map(id) => {
                    for (k, v) in heap.map_entries(id) {
                        self.walk(k, scope, None);
                        self.walk(v, scope, None);
                    }
                    return;
                }
                _ => return,
            }
            let Some(items) = list_items(heap, form) else {
                return;
            };
            let Some(&Value::Sym(head)) = items.first() else {
                for &it in &items {
                    self.walk(it, scope, None);
                }
                return;
            };
            // The privacy expansion's own `(%mark-private 'name)` is bookkeeping, not a use —
            // and so is a `sig`'s registration of its properties (`(sig walk :total)`,
            // ADR-351), which quotes the name it describes.
            if value::symbol_is(head, "%mark-private")
                || value::symbol_is(head, "%register-sig-props")
                || value::symbol_is(head, "%register-sig")
            {
                return;
            }
            if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
                for &it in &items[1..] {
                    self.escape_in_data(it);
                }
                return;
            }
            if self.targets.contains(&head) {
                if self.arity_of(head) == Some(items.len() - 1) {
                    self.out.sites.entry(head).or_default().push(Site::Call(
                        items[1..].to_vec(),
                        Box::new(scope.clone()),
                        self.within,
                    ));
                }
                self.walk_args(form, &items, scope);
                return;
            }
            if value::symbol_is(head, kw::DEF) {
                let name = match items.get(1) {
                    Some(&Value::Sym(n)) => Some(n),
                    _ => None,
                };
                for &it in items.iter().skip(2) {
                    self.walk(it, scope, name);
                }
                return;
            }
            // The scope a site sits in must be what the WALK sees there, or a caller's
            // `(if (nil? j) nil (f j))` hands `f` a `nil | int` the walk knows is `int` —
            // and strict then reports every arithmetic on it inside `f`. Same guard rule
            // (`guards::branch_scopes`), same diverging-guard sequencing over a body.
            if value::symbol_is(head, kw::IF) && (items.len() == 3 || items.len() == 4) {
                self.walk(items[1], scope, None);
                let (then_scope, else_scope) = super::guards::branch_scopes(heap, items[1], scope);
                self.walk(items[2], &then_scope, None);
                if let Some(&else_form) = items.get(3) {
                    self.walk(else_form, &else_scope, None);
                }
                return;
            }
            if value::symbol_is(head, kw::DO) {
                let mut seq = scope.clone();
                for &it in &items[1..] {
                    self.walk(it, &seq, None);
                    if let Some(next) = super::walk::diverging_guard_scope(heap, it, &seq) {
                        seq = next;
                    }
                }
                return;
            }
            if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
                let Some(binds) = items.get(1).and_then(|&b| super::walk::bindings(heap, b)) else {
                    for &it in &items[1..] {
                        self.walk(it, scope, None);
                    }
                    return;
                };
                let mut inner = scope.clone();
                // The pre-binding `check_let` does: `letrec` every name, `let` its
                // `fn`-valued ones (a closure resolves its own name by late lookup).
                for pair in binds.chunks(2) {
                    if let (Some(&Value::Sym(n)), Some(&rhs)) = (pair.first(), pair.get(1))
                        && (value::symbol_is(head, kw::LETREC)
                            || super::walk::fn_form_items(heap, rhs).is_some())
                    {
                        inner = inner.bind(n, None);
                    }
                }
                // A `fn`-valued binder's parameters are derived from its callers — the
                // later bindings and the body — as `check_let` derives them (ADR-355), and
                // its body is walked under them: a site inside `(let (step (fn (l) … (f
                // (+ l dir)))) … (step line))` hands `f` the `int` the walk sees there,
                // not the `number` an unknown `l` makes of the arithmetic. The sites are
                // typed in the scope every binder is bound in, built once ahead.
                let full = if binds.chunks(2).any(|pair| {
                    matches!(pair.first(), Some(Value::Sym(_)))
                        && pair
                            .get(1)
                            .is_some_and(|&rhs| super::walk::fn_form_items(heap, rhs).is_some())
                }) {
                    let mut full = inner.clone();
                    for pair in binds.chunks(2) {
                        let (Some(&pat), Some(&rhs)) = (pair.first(), pair.get(1)) else {
                            continue;
                        };
                        let rhs_ty = super::infer::with_fresh_depth(|| expr_ty(heap, rhs, &full));
                        full = super::walk::let_bind_scope(heap, full, pat, rhs, rhs_ty);
                    }
                    Some(full)
                } else {
                    None
                };
                for (index, pair) in binds.chunks(2).enumerate() {
                    let (Some(&pat), Some(&rhs)) = (pair.first(), pair.get(1)) else {
                        continue;
                    };
                    let derived = match (pat, full.as_ref()) {
                        (Value::Sym(name), Some(full))
                            if super::walk::fn_form_items(heap, rhs).is_some() =>
                        {
                            let visible: Vec<Value> = binds[index * 2 + 2..]
                                .iter()
                                .skip(1)
                                .step_by(2)
                                .copied()
                                .chain(items[2..].iter().copied())
                                .collect();
                            super::walk::derived_let_lambda(heap, name, rhs, &visible, full)
                        }
                        _ => None,
                    };
                    self.pending_fn_params = derived.as_ref().map(|(tys, _)| tys.clone());
                    self.walk(rhs, &inner, None);
                    self.pending_fn_params = None;
                    let rhs_ty = derived
                        .as_ref()
                        .and_then(|(tys, ret)| super::walk::derived_let_lambda_arrow(tys, ret))
                        .or_else(|| super::infer::with_fresh_depth(|| expr_ty(heap, rhs, &inner)));
                    // One rule set with `check_let` (`let_bind_scope`): a guard result
                    // stored in a temporary — `and`'s `(let (g (int? y)) (if g …))` —
                    // narrows `y` at a site under the `if`, as it does in the walk.
                    inner = super::walk::let_bind_scope(heap, inner, pat, rhs, rhs_ty);
                }
                for &body in &items[2..] {
                    self.walk(body, &inner, None);
                    if let Some(next) = super::walk::diverging_guard_scope(heap, body, &inner) {
                        inner = next;
                    }
                }
                return;
            }
            if super::walk::is_fn_head(head) {
                let mut inner = scope.clone();
                let params = items
                    .get(1)
                    .map(|&p| super::walk::fn_params(heap, p))
                    .unwrap_or_default();
                let single_arm = fixed_arms_of_form(heap, form).is_some_and(|arms| arms.len() == 1);
                // What the WALK binds this body's parameters to: a DECLARED sig's types,
                // seeded by the one rule `check_fn_seeded` uses (`seeded_param_types` —
                // an `&optional` position as its declared type or default, a `& rest`
                // binder as its list, a sig naming only the required positions still
                // seeding those); else the derived types — a public function's stay
                // unknown. Only a single plain arm derives positionally.
                let declared: Option<Vec<Option<Ty>>> =
                    match (def_of.and_then(|n| self.ctx.declared_sig(n)), items.get(1)) {
                        (Some(sig), Some(&params_form)) => {
                            let (seeded, fits) = super::walk::seeded_param_types(
                                heap,
                                params_form,
                                Some(&sig),
                                self.ctx,
                            );
                            fits.map(|_| seeded.into_iter().map(|(ty, _)| ty).collect())
                        }
                        _ => None,
                    };
                // …or, for a `let`-bound literal, what its callers hand it (the `let` arm
                // left them for this form alone — taken here so a nested literal never
                // inherits them).
                let let_derived = self.pending_fn_params.take();
                let bound: Option<Vec<Option<Ty>>> = declared.or_else(|| {
                    def_of
                        .and_then(|n| self.derived.get(&n).cloned())
                        .or(let_derived)
                        .filter(|d| single_arm && d.len() == params.len())
                });
                for (i, p) in params.iter().enumerate() {
                    let ty = bound.as_ref().and_then(|d| d.get(i).cloned().flatten());
                    inner = inner.bind(*p, ty);
                }
                // …and the count relations its callers establish (ADR-350), so a self-call
                // under `(< i n)` sees `i` bounded by `xs`, as the walk will.
                let outer_within = self.within;
                if let Some(name) = def_of.filter(|_| single_arm) {
                    inner = inner.with_derived_count_aliases(name, &params);
                    self.within = Some(name);
                }
                for &it in &items[1..] {
                    self.walk(it, &inner, None);
                }
                self.within = outer_within;
                return;
            }
            if super::walk::resolves_to_macro(heap, self.ctx, head) {
                // Syntax the expander left as written: it may construct a call the walk
                // cannot see, so the file's closed-caller premise is gone.
                if crate::diagnostics::debug_flags::derive_debug() {
                    eprintln!(
                        "[derive] OPAQUE: unexpanded macro call ({} …)",
                        value::symbol_name_ref(head)
                    );
                }
                self.opaque = true;
                return;
            }
            self.walk_args(form, &items, scope);
        }
        /// The parameter count of candidate `name`'s one arm.
        fn arity_of(&self, name: Symbol) -> Option<usize> {
            self.candidates
                .get(&name)
                .and_then(|&f| fixed_arms_of_form(self.heap, f))
                .and_then(|arms| arms.first().map(|a| a.heads.len()))
        }
        /// Walk a call's arguments. A bare candidate among them is a HANDOVER when the
        /// callee promises what it will call it with (`walk::callback_seed`: a fold, an
        /// element combinator, a declared or inferred arrow) — recorded as a site of those
        /// parameter types, and not walked, since walked it would read as an escape.
        fn walk_args(&mut self, form: Value, items: &[Value], scope: &Ctx) {
            let handed = super::walk::callback_seed(
                self.heap,
                form,
                items,
                scope,
                &|arg, wanted| matches!(arg, Value::Sym(s) if self.targets.contains(&s) && self.arity_of(s) == Some(wanted)),
            );
            // …and a `fn` LITERAL in a callback position is walked under the parameters
            // the callee promises it — a fold's accumulator and element, `map`'s element,
            // a declared arrow — as the walk binds it (`check_call`'s seeding). Unseeded,
            // a site inside `(fold segs {:col x} (fn (a s) … (:col a) …))` handed its
            // callee `number` for an accumulator field the walk knew was `int`.
            // Asked only when an argument IS a literal: the fold seed is the fold's own
            // result inference (a fixpoint), and this runs at every call form on every
            // derivation round — asked unconditionally it took bedit's `commands.blsp`
            // from a second to twenty-seven.
            let has_literal = items[1..].iter().any(|&arg| {
                list_items(self.heap, arg)
                    .and_then(|l| l.first().copied())
                    .is_some_and(|h| matches!(h, Value::Sym(s) if super::walk::is_fn_head(s)))
            });
            let literal = if has_literal {
                super::walk::callback_seed(
                    self.heap,
                    form,
                    items,
                    scope,
                    &super::walk::literal_fits(self.heap),
                )
            } else {
                None
            };
            // A quoted datum handed straight to a PRINTER is text, not a value anything
            // could call: `(pr-str (quote (assert= 1500 (drive 3000 0))))` is what every
            // assertion macro expands to for its failure message, and reading `drive`
            // there as an escape excluded every private function under a test from
            // derivation — a driver called only from its tests derived from its own
            // recursion alone and reported `number` on `(- i 1)`.
            let prints = matches!(items[0], Value::Sym(s)
                if !scope.is_lexical_local(s)
                    && (value::symbol_is(s, "pr-str") || value::symbol_is(s, "str")));
            for (i, &it) in items.iter().enumerate().skip(1) {
                if prints
                    && list_items(self.heap, it)
                        .and_then(|l| l.first().copied())
                        .is_some_and(
                            |h| matches!(h, Value::Sym(s) if value::symbol_is(s, kw::QUOTE)),
                        )
                {
                    continue;
                }
                match &handed {
                    Some((index, sig)) if *index == i => {
                        let Value::Sym(name) = it else { continue };
                        self.out
                            .sites
                            .entry(name)
                            .or_default()
                            .push(Site::Handover(sig.params.clone()));
                    }
                    _ => {
                        if let Some((index, sig)) = &literal
                            && *index == i
                        {
                            self.pending_fn_params =
                                Some(sig.params.iter().cloned().map(Some).collect());
                        }
                        self.walk(it, scope, None);
                        self.pending_fn_params = None;
                    }
                }
            }
        }
    }
    let mut walker = Walker {
        heap,
        targets,
        candidates,
        derived,
        ctx,
        out: PrivateSites {
            sites: HashMap::new(),
            escaped: HashSet::new(),
        },
        opaque: false,
        within: None,
        pending_fn_params: None,
    };
    walker.walk(form, ctx, None);
    if walker.opaque {
        return None;
    }
    Some(walker.out)
}

/// The **count relations** between a live private function's parameters that its call
/// sites establish (ADR-350): `(n, xs)` when every EXTERNAL site hands `n` the count of what
/// it hands `xs` — `(count b)` (or `%string-length`/`vector-length`) beside `b`, or a count
/// alias of `b` beside `b`: one a `let` bound, or one the CALLER's own parameters carry —
/// and every self-call hands the pair on under its own assumption (`n` and `xs`
/// themselves, typically).
///
/// A LEAST fixpoint climbed from the direct sites: round `k+1` reads every site with round
/// `k`'s relations seeded into the bodies, so a relation `tokens` establishes for
/// `tokens-loop` reaches `token-hit` the round after, and `longest-loop` the round after
/// that — the shape `std/regex`'s scan has, four calls deep. Each round's candidates are
/// the pairs every external site establishes under that seed; the self-sites are then read
/// with the candidates assumed as well, and a pair a self-call does not hand on is
/// dropped. Sound by induction on the call chain: a site in a caller's body hands on a
/// relation the caller's own activation had (the previous round's answer), a self-call
/// preserves it under the assumption that its activation had it. A handover site
/// establishes nothing. Purely syntactic — no type is read — so it runs once, before the
/// typed fixpoints, which then see `(nth xs i)` under `(< i n)` as an element.
pub(in crate::types::check) fn derive_count_aliases(
    heap: &Heap,
    forms: &[Value],
    candidates: &HashMap<Symbol, Value>,
    live: &HashMap<Symbol, usize>,
    ctx: &mut Ctx,
) {
    let targets: HashSet<Symbol> = candidates.keys().copied().collect();
    let all_pairs = |arity: usize| -> Vec<(usize, usize)> {
        (0..arity)
            .flat_map(|n| (0..arity).filter(move |&xs| xs != n).map(move |xs| (n, xs)))
            .collect()
    };
    let is_self_site = |site: &Site, name: Symbol| matches!(site, Site::Call(_, _, Some(within)) if *within == name);
    let mut relations: HashMap<Symbol, Vec<(usize, usize)>> = HashMap::new();
    // Every walk here is whole: the relations set on `ctx` between walks are what the
    // walk's scopes are built from, and the cache does not track them.
    let mut cache = SiteCache::new(heap, forms, candidates);
    let none: HashMap<Symbol, Vec<Option<Ty>>> = HashMap::new();
    for _ in 0..MAX_DERIVE_ROUNDS {
        // The external sites, under the previous round's relations.
        ctx.set_derived_count_aliases(relations.clone());
        let Some(collected) = collect_private_sites(
            heap, forms, &targets, candidates, &none, ctx, &mut cache, None,
        ) else {
            relations.clear();
            break;
        };
        let mut candidates_now: HashMap<Symbol, Vec<(usize, usize)>> = HashMap::new();
        for (&name, &arity) in live {
            let sites = collected.sites_of(name);
            if sites.is_empty() {
                continue;
            }
            let external: Vec<&Site> = sites
                .iter()
                .copied()
                .filter(|site| !is_self_site(site, name))
                .collect();
            if arity < 2 || external.is_empty() || collected.escaped(name) {
                continue;
            }
            let pairs: Vec<(usize, usize)> = all_pairs(arity)
                .into_iter()
                .filter(|&(n, xs)| external.iter().all(|site| site.counts(heap, n, xs)))
                .collect();
            if !pairs.is_empty() {
                candidates_now.insert(name, pairs);
            }
        }
        // The self-sites, under the candidates assumed as well.
        ctx.set_derived_count_aliases(candidates_now.clone());
        let Some(collected) = collect_private_sites(
            heap, forms, &targets, candidates, &none, ctx, &mut cache, None,
        ) else {
            relations.clear();
            break;
        };
        let mut next: HashMap<Symbol, Vec<(usize, usize)>> = HashMap::new();
        for (name, pairs) in &candidates_now {
            let sites = collected.sites_of(*name);
            if sites.is_empty() {
                continue;
            }
            let kept: Vec<(usize, usize)> = pairs
                .iter()
                .copied()
                .filter(|&(n, xs)| {
                    sites
                        .iter()
                        .filter(|site| is_self_site(site, *name))
                        .all(|site| site.counts(heap, n, xs))
                })
                .collect();
            if !kept.is_empty() {
                next.insert(*name, kept);
            }
        }
        let settled = next == relations;
        relations = next;
        if settled {
            break;
        }
    }
    ctx.set_derived_count_aliases(relations);
}

/// The candidates [`caller_derived_params`] will derive — those with a call site and no
/// escape — or `None` when the file holds an unexpanded macro call. Type-independent (a
/// site is an arity match, an escape a spelling), so the caller-derived parameter pass can learn it BEFORE it floors
/// any return: a function not in this set keeps same-file inference's return through the whole
/// iteration, since its in-file callers read that return and a ⊥ there would make their
/// derived parameters under-approximate.
pub(in crate::types::check) fn live_private_functions(
    heap: &Heap,
    forms: &[Value],
    candidates: &HashMap<Symbol, Value>,
    ctx: &Ctx,
) -> Option<HashMap<Symbol, usize>> {
    let targets: HashSet<Symbol> = candidates.keys().copied().collect();
    let mut cache = SiteCache::new(heap, forms, candidates);
    let first = collect_private_sites(
        heap,
        forms,
        &targets,
        candidates,
        &HashMap::new(),
        ctx,
        &mut cache,
        None,
    )?;
    // A self-call is not a site that seeds anything: the least fixpoint starts every
    // parameter at ⊥ and a self-call's arguments are typed under those very parameters,
    // so a function whose ONLY sites are its own recursion derives ⊥ for every parameter,
    // its body reads as dead code, and nothing in it — not even `(%string-length 5)` —
    // is ever reported. Its callers are not all here (a public function's
    // never are; a private one's would be a bug of another kind), so it is site-less.
    Some(
        first
            .names()
            .into_iter()
            .filter_map(|name| {
                let sites = first.sites_of(name);
                let live = !first.escaped(name)
                    && sites
                        .iter()
                        .any(|site| !matches!(site, Site::Call(_, _, Some(w)) if *w == name));
                live.then(|| (name, sites[0].arity()))
            })
            .collect(),
    )
}
