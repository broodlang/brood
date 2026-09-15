//! Declared function properties (ADR-351): what a `(sig name … :pure :total)` holds a
//! function to, checked here.
//!
//! - **`:pure`** — the function performs no effect: no message or process operation, no
//!   I/O, no `Table` write (or read — a table is the one mutable value, so reading one is
//!   reading the world), no clock, environment or randomness. Checked by walking the
//!   expanded body for an effectful head, THROUGH the functions it calls — a same-file
//!   function by its form, a loaded one by its closure's arms, a `:pure`-declared callee
//!   trusted (it is checked at its own definition). A primitive is effectful by name
//!   ([`effectful_head`]): a deny-list, so the check is sound in the direction the checker
//!   promises — a reported effect is one the body reaches — and misses what the list does
//!   not name. A `fn` literal handed to a call is walked (a callback runs); one returned
//!   or bound is not (a pure function may build an effectful closure without running it).
//! - **`:total`** — the function terminates and covers its cases. Termination: every
//!   self-call hands SOME one parameter a structural decrease of itself — `(rest p)` /
//!   `(but-last p)` of a `p` the branch knows is non-empty; `(- p k)` / `(dec p)` of a `p`
//!   the branch knows is bounded below; `(+ p k)` / `(inc p)` of a `p` the branch bounds
//!   above, by a literal or by an immutable collection's count — read in the branch scope
//!   the call sits in (`sigs::self_call_sites`). Coverage: the walk reports a `match` failure
//!   the body can reach that it cannot prove unreachable (`walk.rs`, via `Ctx::total_fn`).
//!   Non-tail self-recursion is the existing lint's business. What `:total` does NOT
//!   claim: that the functions it calls terminate, or that no `throw` runs — a `total`
//!   function may raise on purpose; what it may not do is fall into a case it did not
//!   write.
//!
//! One consumer is checked without a declaration: `ui-memo` (ADR-336) caches a view
//! fragment on the assumption its thunk is pure, so a thunk that performs an effect is
//! reported at the `ui-memo` call.

use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Value};
use crate::error::Pos;

use super::ctx::Ctx;
use super::sigs::{declared_heap_sig, self_call_sites};
use super::walk::{fn_params, is_fn_head, list_items};

/// Entry: the declared properties of every `(def name (fn …))` in the expanded forms, and
/// every `ui-memo` thunk.
pub(super) fn check_properties(
    heap: &Heap,
    expanded: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let mut defs: HashMap<Symbol, Value> = HashMap::new();
    let mut allowed: HashMap<Symbol, Vec<String>> = HashMap::new();
    for &form in expanded {
        collect_fn_defs(heap, form, &[], &mut defs, &mut allowed);
    }
    let mut purity = Purity {
        heap,
        ctx,
        defs: &defs,
        memo: HashMap::new(),
        trail: HashSet::new(),
    };
    let mut names: Vec<Symbol> = defs.keys().copied().collect();
    names.sort_by_key(|s| value::symbol_name(*s));
    for name in names {
        let fn_form = defs[&name];
        let pos = heap.form_pos_only(fn_form);
        let allows = |category: &str| {
            allowed
                .get(&name)
                .is_some_and(|c| c.iter().any(|a| a == category))
        };
        if has_prop(heap, ctx, name, "pure") && !allows("pure") {
            // Its own body, not `effect_of_global` — which trusts a `:pure` declaration,
            // and this is where that declaration is earned.
            let bodies = fn_bodies(heap, fn_form);
            if let Some(effect) = purity.effect_in_forms(&bodies) {
                out.push((
                    pos,
                    format!(
                        "{} is declared :pure but performs an effect: {effect}",
                        value::symbol_name(name)
                    ),
                ));
            }
        }
        if has_prop(heap, ctx, name, "total") && !allows("total") {
            if let Some(msg) = non_decreasing_self_call(heap, ctx, name, fn_form) {
                out.push((pos, msg));
            }
        }
    }
    for &form in expanded {
        check_memo_thunks(heap, form, &mut purity, out);
    }
}

/// The category a `(%lint-allow :category …)` marker names — what `check-allow` expands
/// to, and what survives expansion — or `None` for any other form.
fn lint_allow_category(items: &[Value]) -> Option<String> {
    match items {
        [Value::Sym(h), Value::Keyword(category), ..] if value::symbol_is(*h, "%lint-allow") => {
            Some(value::symbol_name(*category))
        }
        _ => None,
    }
}

/// Does `name` carry the property `prop` — declared in this file's `(sig …)`, or in the
/// heap store a loaded module registered it in?
pub(super) fn has_prop(heap: &Heap, ctx: &Ctx, name: Symbol, prop: &str) -> bool {
    ctx.declared_props(name)
        .iter()
        .any(|k| value::symbol_is(*k, prop))
        || super::deps::obs_declared_sig_props(heap, name)
            .iter()
            .any(|k| value::symbol_is(*k, prop))
}

/// Every `(def name (fn …))` in `form`, at the top or under a `do` — with the
/// `check-allow` categories it sits under (`allowed`), which opt it out of that check.
fn collect_fn_defs(
    heap: &Heap,
    form: Value,
    under: &[String],
    out: &mut HashMap<Symbol, Value>,
    allowed: &mut HashMap<Symbol, Vec<String>>,
) {
    // Deep-form stack safety: a `(do (do …))` chain is as deep as its generator made it.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_fn_defs_inner(heap, form, under, out, allowed)
    })
}

fn collect_fn_defs_inner(
    heap: &Heap,
    form: Value,
    under: &[String],
    out: &mut HashMap<Symbol, Value>,
    allowed: &mut HashMap<Symbol, Vec<String>>,
) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    if let Some(category) = lint_allow_category(&items) {
        let mut inner = under.to_vec();
        inner.push(category);
        for &it in &items[2..] {
            collect_fn_defs(heap, it, &inner, out, allowed);
        }
        return;
    }
    if value::symbol_is(head, kw::DO) {
        for &it in &items[1..] {
            collect_fn_defs(heap, it, under, out, allowed);
        }
        return;
    }
    if value::symbol_is(head, kw::DEF) {
        if let (Some(&Value::Sym(name)), Some(&value)) = (items.get(1), items.get(2)) {
            if matches!(list_items(heap, value).as_deref(), Some([Value::Sym(h), ..]) if is_fn_head(*h))
            {
                out.insert(name, value);
                if !under.is_empty() {
                    allowed.insert(name, under.to_vec());
                }
            }
        }
    }
}

// ---- :pure ----------------------------------------------------------------------------

/// Heads that perform an effect, by exact name — the guard lint's list plus what a memo
/// cache cares about that a guard does not: receiving, the clock, randomness, a fresh
/// symbol, a registry write.
const EFFECTFUL_EXACT: &[&str] = &[
    "receive",
    "%receive",
    "spawn-monitor",
    "gensym",
    "provide",
    "%registry-update!",
    "%swap-registry!",
    "%registry-cas!",
    "%isolate",
    "datetime/utc-now",
    "datetime/today",
    "tempo/today",
    // The kernel's own I/O, clock, environment and randomness, which the prelude reaches
    // for directly (`(%write-out …)` is how a warning is printed).
    "%write-out",
    "%write-err",
    "%getenv",
    "%env-all",
    "%now",
    "%random-bytes",
    "%random-token",
    "%clipboard-get",
    "%clipboard-set",
];

/// Whole namespaces that are effects: I/O, the OS, the network, the mutable table,
/// processes and timers, randomness, the tooling that loads and evaluates code, the
/// terminal and the GUI, logging and telemetry.
const EFFECTFUL_PREFIXES: &[&str] = &[
    "io/",
    "file/",
    "os/",
    "tcp/",
    "tls/",
    "http/",
    "sse/",
    "dns/",
    "table/",
    "proc/",
    "timer/",
    "node/",
    "rand/",
    "reflect/",
    "term/",
    "gui/",
    "audio/",
    "log/",
    "telemetry/",
    "task/",
    "gen/",
    "agent/",
    "supervisor/",
    "wasm/",
    "crypto/random",
    "crypto/keypair",
];

/// Is a call to `name` an effect?
fn effectful_head(name: &str) -> bool {
    super::guard_effects::EFFECTFUL_IN_GUARD.contains(&name)
        || EFFECTFUL_EXACT.contains(&name)
        || EFFECTFUL_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The purity walk's state: the file's own definitions, and a memo of each global's
/// verdict (with the trail of globals under examination, so a recursive function is
/// assumed pure while its own body is walked — the coinductive reading, which is the
/// right one: a cycle of calls is not an effect).
struct Purity<'a> {
    heap: &'a Heap,
    ctx: &'a Ctx,
    defs: &'a HashMap<Symbol, Value>,
    memo: HashMap<Symbol, Option<String>>,
    trail: HashSet<Symbol>,
}

impl Purity<'_> {
    /// The effect a call to the global `name` reaches, described, or `None`.
    fn effect_of_global(&mut self, name: Symbol) -> Option<String> {
        if let Some(known) = self.memo.get(&name) {
            return known.clone();
        }
        if !self.trail.insert(name) {
            return None;
        }
        let verdict = self.effect_of_global_uncached(name);
        self.trail.remove(&name);
        self.memo.insert(name, verdict.clone());
        verdict
    }

    fn effect_of_global_uncached(&mut self, name: Symbol) -> Option<String> {
        let spelled = value::symbol_name(name);
        if effectful_head(&spelled) {
            return Some(format!("({spelled} …)"));
        }
        // A declared-pure callee is trusted here and checked at its own definition.
        if has_prop(self.heap, self.ctx, name, "pure") {
            return None;
        }
        if let Some(&fn_form) = self.defs.get(&name) {
            let bodies = fn_bodies(self.heap, fn_form);
            return self
                .effect_in_forms(&bodies)
                .map(|e| format!("calls {spelled}, which performs an effect: {e}"));
        }
        if self.ctx.is_file_global(name) {
            return None;
        }
        // A loaded closure: its arms' bodies, as the heap holds them.
        let Value::Fn(cid) = super::deps::obs_global(self.heap, name)? else {
            return None;
        };
        let closure = self.heap.closure(cid);
        let bodies: Vec<Value> = closure
            .arms
            .iter()
            .flat_map(|arm| arm.body.iter().copied())
            .collect();
        self.effect_in_forms(&bodies)
            .map(|e| format!("calls {spelled}, which performs an effect: {e}"))
    }

    fn effect_in_forms(&mut self, forms: &[Value]) -> Option<String> {
        forms
            .iter()
            .find_map(|&f| self.effect_in(f, &HashSet::new()))
    }

    /// The first effect `form` reaches, with `locals` the names bound around it (a call
    /// through a local is an unknown callee, not an effect).
    fn effect_in(&mut self, form: Value, locals: &HashSet<Symbol>) -> Option<String> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            self.effect_in_inner(form, locals)
        })
    }

    fn effect_in_inner(&mut self, form: Value, locals: &HashSet<Symbol>) -> Option<String> {
        let heap = self.heap;
        match form {
            Value::Vector(id) => {
                let items = heap.vector(id).to_vec();
                return items.iter().find_map(|&it| self.effect_in(it, locals));
            }
            Value::Map(id) => {
                let entries = heap.map_entries(id);
                return entries.iter().find_map(|&(k, v)| {
                    self.effect_in(k, locals)
                        .or_else(|| self.effect_in(v, locals))
                });
            }
            Value::Pair(_) => {}
            _ => return None,
        }
        let items = list_items(heap, form)?;
        let Some(&Value::Sym(head)) = items.first() else {
            return items.iter().find_map(|&it| self.effect_in(it, locals));
        };
        let head_name = value::symbol_name(head);
        if head_name == kw::QUOTE || head_name == kw::QUASIQUOTE {
            return None;
        }
        // A binding form extends the locals; a global-defining form IS an effect.
        if value::symbol_is(head, kw::DEF) || value::symbol_is(head, kw::DEFMACRO) {
            return Some(format!("({head_name} …)"));
        }
        if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
            let mut inner = locals.clone();
            if let Some(binds) = items.get(1).and_then(|&b| super::walk::bindings(heap, b)) {
                for pair in binds.chunks(2) {
                    if let Some(&rhs) = pair.get(1) {
                        if let Some(e) = self.effect_in(rhs, &inner) {
                            return Some(e);
                        }
                    }
                    if let Some(&pat) = pair.first() {
                        collect_pattern_names(heap, pat, &mut inner);
                    }
                }
            }
            return items[2..].iter().find_map(|&it| self.effect_in(it, &inner));
        }
        if is_fn_head(head) {
            // A `fn` literal here is a VALUE (a tail, a binding) — not run by this body.
            return None;
        }
        if head_name == kw::IF || head_name == kw::DO {
            return items[1..].iter().find_map(|&it| self.effect_in(it, locals));
        }
        // A call. Its head first, then each argument — where a `fn` literal argument IS
        // walked: a callback handed to a call runs.
        if !locals.contains(&head) {
            if let Some(e) = self.effect_of_global(head) {
                return Some(e);
            }
        }
        for &arg in &items[1..] {
            let found = match list_items(heap, arg).as_deref() {
                Some([Value::Sym(h), ..]) if is_fn_head(*h) => {
                    let mut inner = locals.clone();
                    for p in fn_params(heap, list_items(heap, arg).unwrap()[1]) {
                        inner.insert(p);
                    }
                    let bodies = fn_bodies(heap, arg);
                    bodies.iter().find_map(|&b| self.effect_in(b, &inner))
                }
                _ => self.effect_in(arg, locals),
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
}

/// The body forms of a `(fn …)` form — every arm's, for a multi-arity one.
fn fn_bodies(heap: &Heap, fn_form: Value) -> Vec<Value> {
    let Some(items) = list_items(heap, fn_form) else {
        return Vec::new();
    };
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        let clauses = match items.get(1..) {
            Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
            Some(rest) => rest,
            None => return Vec::new(),
        };
        return clauses
            .iter()
            .filter_map(|&c| list_items(heap, c))
            .flat_map(|citems| citems.into_iter().skip(1))
            .collect();
    }
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    items
        .get(body_start..)
        .map(<[Value]>::to_vec)
        .unwrap_or_default()
}

/// Every symbol a binding pattern binds (a bare name, or the names inside a
/// destructuring vector/list/map).
fn collect_pattern_names(heap: &Heap, pat: Value, out: &mut HashSet<Symbol>) {
    let mut work = vec![pat];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) => {
                out.insert(s);
            }
            Value::Pair(_) => {
                if let Some(items) = list_items(heap, v) {
                    work.extend(items);
                }
            }
            Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
            Value::Map(id) => {
                for (_, val) in heap.map_entries(id) {
                    work.push(val);
                }
            }
            _ => {}
        }
    }
}

/// A `(ui-memo key deps thunk)` call whose thunk performs an effect is reported: the memo
/// reuses the fragment while `deps` are equal, so the effect runs on some turns and not
/// others — the one thing a cached view must not do.
fn check_memo_thunks(
    heap: &Heap,
    form: Value,
    purity: &mut Purity<'_>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        let Some(items) = list_items(heap, v) else {
            if let Value::Vector(id) = v {
                work.extend(heap.vector(id).iter().copied());
            }
            continue;
        };
        if let Some(&Value::Sym(head)) = items.first() {
            let name = value::symbol_name(head);
            if name == kw::QUOTE || name == kw::QUASIQUOTE {
                continue;
            }
            // `(check-allow :pure …)`: a thunk that effects on purpose (a test counting
            // its own recomputations by sending from it).
            if lint_allow_category(&items).as_deref() == Some("pure") {
                continue;
            }
            if name.rsplit('/').next() == Some("ui-memo") && items.len() == 4 {
                let thunk = items[3];
                let effect = match list_items(heap, thunk).as_deref() {
                    Some([Value::Sym(h), ..]) if is_fn_head(*h) => {
                        let bodies = fn_bodies(heap, thunk);
                        purity.effect_in_forms(&bodies)
                    }
                    _ => match thunk {
                        Value::Sym(g) if !purity.ctx.is_local(g) => purity.effect_of_global(g),
                        _ => None,
                    },
                };
                if let Some(effect) = effect {
                    out.push((
                        heap.form_pos_only(v),
                        format!(
                            "ui-memo caches a fragment whose thunk performs an effect: {effect} \
                             — a cached fragment is reused while its deps are equal, so the \
                             effect runs on some turns and not others"
                        ),
                    ));
                }
            }
        }
        work.extend(items);
    }
}

// ---- :total ---------------------------------------------------------------------------

/// The termination half of `:total`: a self-call that hands no parameter a structural
/// decrease of itself, described — or `None` when every self-call does, or there is none.
fn non_decreasing_self_call(
    heap: &Heap,
    ctx: &Ctx,
    name: Symbol,
    fn_form: Value,
) -> Option<String> {
    let items = list_items(heap, fn_form)?;
    let multi = crate::eval::macros::fn_is_arity_multi_clause(heap, &items);
    let arms: Vec<(Vec<Symbol>, Vec<Value>)> = if multi {
        let clauses = match items.get(1..) {
            Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
            Some(rest) => rest,
            None => return None,
        };
        clauses
            .iter()
            .filter_map(|&c| {
                let citems = list_items(heap, c)?;
                let params = fn_params(heap, *citems.first()?);
                Some((params, citems[1..].to_vec()))
            })
            .collect()
    } else {
        let params = fn_params(heap, *items.get(1)?);
        vec![(params, fn_bodies(heap, fn_form))]
    };
    // This file's own declaration first (the file is not loaded while it is checked), else
    // a loaded one — DECLARED only: an inferred loaded sig is the demand-based one, wider
    // than what the callers here hand it.
    let declared = ctx
        .declared_sig(name)
        .or_else(|| declared_heap_sig(heap, name));
    for (params, bodies) in arms {
        // The body scope: the declared or derived parameter types, so a guard's narrowing
        // of a parameter (`(> n 0)`, `(nil? xs)`) is what the site's scope reads.
        let mut scope = ctx.clone();
        let derived = ctx.derived_params(name).cloned();
        for (index, &p) in params.iter().enumerate() {
            let ty = declared.as_ref().and_then(|s| s.param(index)).or_else(|| {
                derived
                    .as_ref()
                    .and_then(|d| d.get(index).cloned().flatten())
            });
            scope = scope.bind(p, ty);
        }
        scope = scope.with_derived_count_aliases(name, &params);
        let sites: Vec<(Vec<Value>, Ctx)> = bodies
            .iter()
            .flat_map(|&b| self_call_sites(heap, b, name, params.len(), &scope))
            .collect();
        if sites.is_empty() {
            continue;
        }
        let some_position_decreases = (0..params.len()).any(|k| {
            sites
                .iter()
                .all(|(args, site_scope)| decreases(heap, args[k], params[k], site_scope))
        });
        if !some_position_decreases {
            return Some(format!(
                "{} is declared :total but a self-call hands no parameter a structural \
                 decrease of itself — `(rest xs)` of a non-empty `xs`, or `(- n 1)` of an `n` \
                 the branch bounds below — so the checker cannot see it terminate",
                value::symbol_name(name)
            ));
        }
    }
    None
}

/// Is `arg` a structural decrease of the parameter `param`, in the scope the self-call
/// sits in? A shorter sequence of a `param` that is non-empty there; a smaller int of a
/// `param` that is bounded below there.
fn decreases(heap: &Heap, arg: Value, param: Symbol, scope: &Ctx) -> bool {
    let Some(items) = list_items(heap, arg) else {
        return false;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return false;
    };
    if scope.is_lexical_local(head) {
        return false;
    }
    let param_ty = scope.get(param);
    let is_param = |v: &Value| matches!(v, Value::Sym(s) if *s == param);
    let non_empty = || {
        param_ty
            .as_ref()
            .and_then(|t| t.count_range())
            .and_then(|r| r.lo)
            .is_some_and(|lo| lo >= 1)
            && param_ty
                .as_ref()
                .is_some_and(|t| !t.contains_tag(crate::core::value::Tag::Nil))
    };
    let bounded_below = || {
        param_ty
            .as_ref()
            .and_then(|t| t.int_range())
            .and_then(|r| r.lo)
            .is_some()
    };
    // A counter climbing under an upper bound is a descent of `bound - p`: the bound is
    // a literal the branch established (`(< i 100)`) or the count of an immutable
    // collection (`(< i (count xs))`, `Ctx::index_bounds`).
    let bounded_above = || {
        param_ty
            .as_ref()
            .and_then(|t| t.int_range())
            .and_then(|r| r.hi)
            .is_some()
            || scope.is_index_bound_by_any(param)
    };
    let positive_literal = |v: &Value| matches!(v, Value::Int(n) if *n > 0);
    let negative_literal = |v: &Value| matches!(v, Value::Int(n) if *n < 0);
    match (value::symbol_name(head).as_str(), &items[1..]) {
        ("rest" | "but-last", [x]) if is_param(x) => non_empty(),
        ("dec", [x]) if is_param(x) => bounded_below(),
        ("inc", [x]) if is_param(x) => bounded_above(),
        ("-", [x, k]) if is_param(x) && positive_literal(k) => bounded_below(),
        ("-", [x, k]) if is_param(x) && negative_literal(k) => bounded_above(),
        ("+", [x, k]) | ("+", [k, x]) if is_param(x) && negative_literal(k) => bounded_below(),
        ("+", [x, k]) | ("+", [k, x]) if is_param(x) && positive_literal(k) => bounded_above(),
        _ => false,
    }
}
