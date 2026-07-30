//! The recursive walk: visit every sub-form, open the right scope at each
//! binder (`let` / `fn` / `defn` / …), and at every call site cross-check
//! arity + per-argument type against what the callee accepts.
//!
//! Each `check_*` helper for a special form clones its enclosing `Ctx`, adds
//! the binder(s) it introduces, walks its body in that extended scope, and
//! returns — the generic call-form path at the bottom of [`check_into`] runs
//! only for non-special heads. [`fn_params`], [`list_items`], [`bindings`]
//! are the tiny syntax-shape readers the rest of the walk shares; they're
//! `pub(super)` so the sibling submodules (`sigs`, `guards`) can use them.

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::keywords as kw;
use crate::core::value::{self, Arity, Symbol, Value};
use crate::error::Pos;
use crate::types::{GradualTy, Ty};

use super::ctx::{Ctx, PathKey};
use super::guards::{
    find_redundant_clause, guard_assertion, is_syntactic_keyword, literal_eq_test_raw,
    match_exhaustiveness_gap, path_guard_assertion, render_literal_pattern,
};
use super::infer::{expr_ty, global_value_ty};
use super::sigs::{
    arity_of, arity_str, curated_sig, declared_heap_overload, declared_heap_sig,
    declared_heap_value_ty, is_globally_bound, sig_of,
};

/// `symbol_name(s)` is a `String` allocation; we only need the spelling on
/// the rare *error* paths (unbound / arity / type-disjoint). Wrap as a
/// no-arg helper so the hot path (the whole `is_local` / `is_syntactic` /
/// `is_globally_bound` / `curated_sig` short-circuit) skips it entirely.
#[inline]
fn name_of(s: Symbol) -> String {
    value::symbol_name(s)
}

/// Is `s` a **gensym temporary** — a `<prefix>__<digits>` name minted by macro
/// expansion (`value::gensym`)? Such a binding is compiler-introduced, so the
/// lints that only want *surface* (user-written) names — the unused-let-binding
/// lint and the broadened dead-clause lint (ADR-131) — exempt it: warning on a
/// name the user can't rename is noise. (A rare hand-written `x__1` is only a
/// missed warning, which these lints already tolerate.)
fn is_gensym_sym(s: Symbol) -> bool {
    name_of(s)
        .rsplit_once("__")
        .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The finding position for a call **argument**: the argument's own source
/// position when it has one (a nested call / vector — the reader positions
/// pairs, so `(string-length (+ 1 2))` anchors the type finding at `(+ 1 2)`,
/// not the call head), falling back to the whole call form for a bare literal
/// or symbol (which the pair-keyed position table doesn't record). Finer LSP /
/// `nest check` spans without threading `Pos` through the whole walk.
fn arg_pos(heap: &Heap, arg: Value, form: Value) -> Option<crate::error::Pos> {
    heap.form_pos_only(arg).or_else(|| heap.form_pos_only(form))
}

/// The arity of a callback argument, when it can be determined *unambiguously* —
/// the input to the callback-arity check (ADR-078). A named **global** function
/// (its arity lives in the heap) or a simple single-clause lambda literal yields
/// an arity; a local variable (arity unknown here), a multi-clause / pattern /
/// variadic lambda, or any non-function form yields `None` (skip — so the check
/// never produces a false positive).
fn callback_arity(heap: &Heap, arg: Value, ctx: &Ctx) -> Option<Arity> {
    match arg {
        // A local binding shadows the global table — its arity isn't known here.
        Value::Sym(s) if ctx.is_local(s) => None,
        Value::Sym(s) => arity_of(heap, s),
        Value::Pair(_) => lambda_literal_arity(heap, arg),
        _ => None,
    }
}

/// True when `head` is a function-literal head — `fn` or its synonym `lambda`.
/// Both spell the same special form (`lambda` Just Works, see the evaluator), and
/// both survive macro expansion as their original head, so every reader of a `fn`
/// shape (here, [`guards::lambda_ret`], and `protocol`'s arity reader) must accept
/// the two. Single source of truth so they can't drift.
pub(super) fn is_fn_head(head: Symbol) -> bool {
    value::symbol_is(head, kw::FN)
}

/// The arity of a **single-clause** `fn` literal — `(fn (a b) …)` → `exact(2)`,
/// `(fn (a &optional b) …)` → `range(1, 2)`, `(fn (a b & c) …)` → `at_least(2)`.
/// This mirrors what `arity_of` already computes for a *named* variadic global, so
/// a variadic inline lambda whose *minimum* arity exceeds what a fixed-arity HOF
/// supplies (e.g. `(map (fn (a b & c) …) xs)` — needs ≥2, gets 1) is now caught,
/// while a permissive `(fn (& xs) …)` (min 0) still isn't flagged.
///
/// `None` for anything we can't read off cleanly — a multi-arity `fn` (clause
/// lists, not a bare param list), a destructuring parameter, an out-of-order
/// marker, or a non-`fn` head — so the callback-arity check stays
/// false-positive-free.
fn lambda_literal_arity(heap: &Heap, form: Value) -> Option<Arity> {
    let items = list_items(heap, form)?;
    let Some(Value::Sym(head)) = items.first().copied() else {
        return None;
    };
    if !is_fn_head(head) {
        return None;
    }
    // Peel an optional leading docstring, matching the evaluator's `fn` parse.
    let parts = &items[1..];
    let parts = match parts.first() {
        Some(Value::Str(_)) if parts.len() > 1 => &parts[1..],
        _ => parts,
    };
    // The parameter list. A multi-arity `fn` has clause *lists* here instead
    // (`((a) …) ((a b) …)`), whose elements aren't bare symbols → we bail below.
    let params = list_items(heap, *parts.first()?)?;
    // Phase machine over the param list: required names, then an optional run
    // after `&optional`, then a single rest binder after `&`. A marker out of
    // order (or repeated) is a shape we don't model — bail.
    #[derive(PartialEq)]
    enum Phase {
        Required,
        Optional,
        Rest,
    }
    let mut phase = Phase::Required;
    let mut required = 0usize;
    let mut optional = 0usize;
    let mut has_rest = false;
    for p in params {
        let Value::Sym(sym) = p else {
            // A destructuring pattern (nested list/vector) or a clause list →
            // not a simple parameter, so not a shape we count here.
            return None;
        };
        if value::symbol_is(sym, kw::AMP_OPTIONAL) {
            if phase != Phase::Required {
                return None;
            }
            phase = Phase::Optional;
        } else if value::symbol_is(sym, kw::AMP) {
            if phase == Phase::Rest {
                return None;
            }
            phase = Phase::Rest;
            has_rest = true;
        } else {
            match phase {
                Phase::Required => required += 1,
                Phase::Optional => optional += 1,
                Phase::Rest => {} // the single rest binder — name doesn't affect arity
            }
        }
    }
    Some(if has_rest {
        Arity::at_least(required)
    } else if optional > 0 {
        Arity::range(required, required + optional)
    } else {
        Arity::exact(required)
    })
}

/// How a callback argument reads in a diagnostic — a named function by its name, an
/// inline one as "the fn" (it said "the lambda" until ADR-162 retired that spelling;
/// a diagnostic shouldn't name a form the language no longer has).
fn callback_desc(arg: Value) -> String {
    match arg {
        Value::Sym(s) => name_of(s),
        _ => "the fn".to_string(),
    }
}

/// The output sinks the **function-as-value** lint guards. Passing a bare
/// zero-arg function to one of these stringifies the *function* (`#<fn …>`)
/// instead of calling it — the silent `(print ansi-clear)`-for-`(print
/// (ansi-clear))` slip. Four lock-free `symbol_is` compares, only reached on
/// the generic-call path (so no `symbol_name` allocation on the hot path).
fn is_output_sink(s: Symbol) -> bool {
    value::symbol_is(s, "print")
        || value::symbol_is(s, "println")
        || value::symbol_is(s, "str")
        || value::symbol_is(s, "format")
}

/// The `unbound symbol: …` diagnostic text for `nm`, with the foreign-construct
/// hint appended when `nm` names a construct from another Lisp that Brood lacks
/// (so the Brood way is visible at write-time). Shared by the call-head and the
/// value-leaf unbound checks so the two messages can't drift apart.
fn unbound_msg(nm: &str) -> String {
    let mut msg = format!("unbound symbol: {}", nm);
    if let Some(hint) = crate::eval::foreign_construct_hint(nm) {
        msg.push_str(" — ");
        msg.push_str(hint);
    }
    msg
}

/// The debug-only primitives (registered under `#[cfg(debug_assertions)]`, see
/// `builtins/mod.rs` / `builtins/system.rs`): they exist in a dev build but not a
/// release one, so a `nest check` running in a release binary would flag every
/// (legitimate, `bound?`-guarded) test reference as unbound. They're real
/// primitives, so the checker knows their names regardless of the build config —
/// the honest fix for the "release-only phantom unbound" build artifact.
fn is_debug_only_primitive(nm: &str) -> bool {
    matches!(nm, "%blob-ptr" | "%blob-strong-count" | "%force-panic")
}

/// Relax a parameter type for the call-argument membership test, in the two places
/// the type lattice deliberately under-approximates — so the advisory arg-check never
/// misfires on a value that is in fact valid:
///  - a **record-shape** parameter (`(record …)`) drops its **optional** fields,
///    keeping only the required ones. The shape-subtype relation is conservative: a
///    literal `{name}` isn't a subtype of `{name, age?}` even though the value
///    satisfies it (the optional `age` is simply absent), so requiring the optional
///    field's *declaration* would false-flag a valid argument. Dropping optionals
///    keeps the sound part — a missing or wrong-typed *required* field is still caught
///    (so a guard-refined record still flows a real conflict into the call).
///  - a **`list<T>`** parameter also admits the empty list, which the lattice stores
///    as the separate `nil` tag (`Ty::list_of` is `pair`-only by design), so a `nil`
///    argument — the empty list — is consistent with it.
///  - a **callable** parameter (a `(… -> …)` arrow, or the bare `fn | native` of
///    `apply`) also admits a **keyword**, because a keyword IS callable as an
///    accessor (ADR-165): `(map :name people)` is valid, and the lattice has no way
///    to say "keyword, which behaves as (map -> any)" — `Tag::Keyword` and the
///    function tags are disjoint bits. Without this the single most-motivating call
///    for that feature would draw a warning.
fn relax_param_for_arg(param: &Ty) -> Ty {
    use crate::types::Tag;
    let mut p = param.clone();
    if p.contains_tag(Tag::Fn) || p.contains_tag(Tag::Native) {
        p = p.union(Ty::of(Tag::Keyword));
    }
    if let Some(fields) = p.record_fields() {
        if fields.values().any(|(_, required)| !*required) {
            let required_only: std::collections::BTreeMap<_, _> = fields
                .iter()
                .filter(|(_, (_, required))| *required)
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            p = Ty::record_of(required_only);
        }
    }
    if p.contains_tag(Tag::Pair) && p.elem_ty().is_some() {
        p = p.union(Ty::of(Tag::Nil));
    }
    p
}

/// Does `sig`'s arity accept exactly `argc` arguments — its fixed params, plus
/// any `&optional` slots, plus an unbounded `&rest` tail?
fn sig_accepts_argc(sig: &crate::types::Sig, argc: usize) -> bool {
    let min = sig.params.len();
    if argc < min {
        return false;
    }
    sig.rest.is_some() || argc <= min + sig.optional.len()
}

/// ADR-116 completion: does a call with these argument types match **no** arm of
/// a declared overload? False-positive-free by construction — it rules an arm
/// out only when a *known* argument is provably **disjoint** from that arm's
/// parameter (an unknown or `NEVER` arg never rules an arm out), and flags only
/// when *every* arity-relevant arm is ruled out. Arms whose arity can't accept
/// `argc` are left to the separate arity check (so a pure arity mismatch isn't
/// double-reported); if no arm even has a fitting arity we defer entirely.
fn overload_arg_mismatch(sigs: &[crate::types::Sig], arg_tys: &[Option<Ty>]) -> bool {
    let argc = arg_tys.len();
    let mut any_arity_ok = false;
    for sig in sigs {
        if !sig_accepts_argc(sig, argc) {
            continue;
        }
        any_arity_ok = true;
        let possible = arg_tys.iter().enumerate().all(|(i, arg_ty)| match arg_ty {
            // An unknown arg, or a `NEVER` (unreachable-branch) arg, never rules
            // an arm out — matches the single-sig loop's `is_never` skip.
            Some(a) if !a.is_never() => sig.param(i).is_none_or(|p| !a.is_disjoint(&p)),
            _ => true,
        });
        if possible {
            return false; // some arity-relevant arm could accept the call
        }
    }
    any_arity_ok // ≥1 arm had a fitting arity, and every such arm was ruled out
}

/// The suppression bitmask a `(check-allow :category …)` marker's category
/// keyword names. Unknown / missing → `0` (suppress nothing — a typo'd category
/// is thus a no-op that still lints, never a silent blanket suppression). Keep
/// the recognised names in sync with the `check-allow` docstring.
fn lint_allow_mask(category: Option<Value>) -> u8 {
    let Some(Value::Keyword(k)) = category else {
        return 0;
    };
    if value::symbol_is(k, "non-tail-recursion") {
        super::ctx::SUPPRESS_NON_TAIL
    } else if value::symbol_is(k, "unreachable-clause") {
        super::ctx::SUPPRESS_UNREACHABLE
    } else if value::symbol_is(k, "type-mismatch") {
        super::ctx::SUPPRESS_TYPE_MISMATCH
    } else if value::symbol_is(k, "unbound") {
        super::ctx::SUPPRESS_UNBOUND
    } else if value::symbol_is(k, "unrequired") {
        super::ctx::SUPPRESS_UNREQUIRED
    } else {
        0
    }
}

/// A symbol in *reference* position that resolves to nothing — not a local
/// binder, not a syntactic keyword, not a curated stdlib name, and not in the
/// heap's globals (which includes macros and, once the project is loaded,
/// file-local defs). The single predicate behind **both** the call-head and the
/// operand unbound diagnostics, so the two never drift apart.
fn is_unbound(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_local(s) || is_globally_bound(heap, s) || curated_sig(s).is_some() {
        return false;
    }
    let nm = name_of(s);
    if is_syntactic_keyword(&nm) || is_debug_only_primitive(&nm) {
        return false;
    }
    // A *qualified* reference (`mod/name`) whose module we don't know — no `mod/*`
    // is loaded — can't be proven unbound: the module may be defined dynamically
    // (`%load-string`, a required temp module) or live in a file a single-file
    // check didn't load. Stay silent. A typo in a *known* module (some `mod/*`
    // loaded) still falls through to the warning, so real qualified typos are kept.
    if let Some(slash) = nm.rfind('/') {
        // Record the known-ns query for the Phase-2 cache (heap-resident recorder):
        // this file's unbound verdict depends on whether the prefix is known.
        super::deps::obs_known_ns(heap, &nm[..=slash]);
        if !ctx.module_is_known(&nm[..=slash]) {
            return false;
        }
    }
    true
}

/// **KI-17** — a *user-written* qualified reference `mod/name` that resolves in the
/// loaded image but whose module `mod` the file never `require`s/`:use`s. It works only
/// by load-order luck (another file pulled `mod` in first); reorder or drop that file and
/// it raises `unbound symbol: mod/name` at runtime. Returns `Some(mod)` to warn.
///
/// Silent unless the file's reachability set is known ([`Ctx::required_mods`], whole-
/// project mode), the symbol is genuinely *bound* (an unbound one is [`is_unbound`]'s
/// job — the two are mutually exclusive), and the exact reference is *user-written*
/// ([`Ctx::raw_qualified_has`] — never a macro-injected reference to a module the file
/// doesn't mention). Each guard removes a false-positive class, keeping the lint sound.
fn unrequired_module(heap: &Heap, ctx: &Ctx, s: Symbol) -> Option<String> {
    let required = ctx.required_mods()?;
    let nm = name_of(s);
    let slash = nm.rfind('/')?;
    let module = &nm[..slash];
    if module.is_empty() {
        return None; // a bare `/` (division), not a qualified reference
    }
    if required.contains(module) || !ctx.raw_qualified_has(&nm) {
        return None;
    }
    // A local shadow, or a name the loaded image doesn't bind, is not this lint's
    // concern (the latter is `is_unbound`'s).
    if ctx.is_local(s) || !is_globally_bound(heap, s) {
        return None;
    }
    Some(module.to_string())
}

/// The KI-17 reachability diagnostic text for a reference to unrequired `module`.
fn unrequired_msg(module: &str) -> String {
    format!("qualified reference to unrequired module: {module} (add (require '{module}) to this file)")
}

/// True when a call whose head is `s` *evaluates its arguments as values* — `s`
/// resolves to a primitive, a known Brood closure, a curated stdlib fn, or a
/// lexical local (a param / `let` name, never a macro). False for a macro, a
/// special-form keyword, an unknown head, or anything we can't prove is a
/// non-macro callable.
///
/// This gates the operand-unbound check: only when arguments are genuinely
/// evaluated is a bare-symbol operand a *reference* (so an unresolvable one is
/// truly unbound). For a macro or unknown head the operands may be opaque syntax
/// (pattern keywords, quoted tags) or a forward reference, so they're left
/// untouched — preserving the checker's no-false-positives rule.
fn evaluates_args(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_lexical_local(s) {
        return true;
    }
    match super::deps::obs_global(heap, s) {
        Some(Value::Native(_)) | Some(Value::Fn(_)) => true,
        // A `Value::Macro` does NOT evaluate its args; any other bound non-callable
        // isn't a call we should reason about either.
        Some(_) => false,
        // Not in the heap: only the curated stdlib closures count as known callables.
        None => curated_sig(s).is_some(),
    }
}

/// True when call head `s` resolves to a **macro the checker did not expand** — a
/// file-local `defmacro` (single-file mode, or one defined inside a deferred
/// `test`/`describe` thunk) or a `Value::Macro` in the heap. A lexical local
/// shadows any such name, so it isn't a macro then.
///
/// Such a call's arguments are *opaque syntax*: a macro may quote them, splice
/// them into a binder, or `def` a symbol argument — none of which is evaluated
/// code. So the walk must not descend into them (it would false-flag a template
/// like `(let ((a b) v) (+ a b))`'s spliced `(+ a b)`). Only a macro the compile
/// pass *couldn't* expand reaches the walk, so the lost coverage is inherent.
fn resolves_to_macro(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_lexical_local(s) {
        return false;
    }
    ctx.is_file_macro(s) || matches!(super::deps::obs_global(heap, s), Some(Value::Macro(_)))
}

/// Flag a bare-symbol form sitting in an evaluated *value* position when it's
/// unbound, attributing the warning to `parent` (the enclosing call / `def` /
/// `if` / `let` form the reader positioned — a bare operand symbol carries no
/// `Pos` of its own). A no-op for any non-symbol form, which `check_into` walks
/// instead. Shared by the call-operand loop and the `def`/`let`/`if` value
/// slots so every evaluated-leaf site applies the one [`is_unbound`] rule.
fn check_value_leaf(
    heap: &Heap,
    form: Value,
    parent: Value,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    // Operand / value-slot checking is whole-file-only — a bare fragment's free
    // variables are legitimately ambiguous (see `Ctx::check_operands`).
    if !ctx.checks_operands() {
        return;
    }
    if let Value::Sym(s) = form {
        if is_unbound(heap, ctx, s) && !ctx.is_suppressed(super::ctx::SUPPRESS_UNBOUND) {
            out.push((heap.form_pos_only(parent), unbound_msg(&name_of(s))));
        } else if !ctx.is_suppressed(super::ctx::SUPPRESS_UNREQUIRED) {
            if let Some(m) = unrequired_module(heap, ctx, s) {
                out.push((heap.form_pos_only(parent), unrequired_msg(&m)));
            }
        }
    }
}

/// Conservative reachability scan: does `sym` appear as a `Value::Sym`
/// *anywhere* in `form` — recursively, including in binder positions and
/// inside `quote`? Used by the unused-`let`-binding lint. False negatives are
/// acceptable (a shadowed reference counted as "used"); zero false positives.
fn sym_appears_in(heap: &Heap, form: Value, sym: Symbol) -> bool {
    match form {
        Value::Sym(s) => s == sym,
        Value::Pair(pid) => {
            let (car, cdr) = heap.pair(pid);
            sym_appears_in(heap, car, sym) || sym_appears_in(heap, cdr, sym)
        }
        Value::Vector(vid) => heap
            .vector(vid)
            .iter()
            .any(|&v| sym_appears_in(heap, v, sym)),
        // Map literals (`{:k v …}`) are heap maps, not pairs — scan both keys
        // and values, or a binding used only inside a `{…}` (very common: the
        // editor's minibuffer specs, `{:start s :end e}` edit forms) is falsely
        // reported unused, breaking the "false negatives only" invariant.
        Value::Map(mid) => heap
            .map_entries(mid)
            .iter()
            .any(|&(k, v)| sym_appears_in(heap, k, sym) || sym_appears_in(heap, v, sym)),
        _ => false,
    }
}

/// Collect every `Value::Sym` that appears anywhere in `form` — recursively,
/// including binder positions. Used by the unused-`:use` and unused-private-`defn`
/// lints to build the full reference set of a file in one pass.
fn collect_syms_into(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    // Deep-form stack safety — same stacker remedy as the walkers above.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_syms_into_inner(heap, form, out)
    })
}

fn collect_syms_into_inner(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    match form {
        Value::Sym(s) => {
            out.insert(s);
        }
        Value::Pair(pid) => {
            let (car, cdr) = heap.pair(pid);
            collect_syms_into(heap, car, out);
            collect_syms_into(heap, cdr, out);
        }
        Value::Vector(vid) => {
            for &v in heap.vector(vid).iter() {
                collect_syms_into(heap, v, out);
            }
        }
        Value::Map(mid) => {
            for (k, v) in heap.map_entries(mid) {
                collect_syms_into(heap, k, out);
                collect_syms_into(heap, v, out);
            }
        }
        _ => {}
    }
}

/// Collect every symbol that appears anywhere in `forms`.
pub(super) fn collect_all_syms(heap: &Heap, forms: &[Value]) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    for &form in forms {
        collect_syms_into(heap, form, &mut out);
    }
    out
}

/// What the walk does at a head symbol. `Generic` is the fall-through for any
/// head that isn't one of the recognised special forms / skip-body markers —
/// the walk treats it as a normal call (resolves sig + arity, checks for
/// unbound). One [`SymbolMap`] lookup decides: pre-consolidation each call
/// allocated a `String` via `value::symbol_name` just to feed a chain of
/// `matches!(name.as_str(), "if" | …)` plus `skips_body(&name)` — that was
/// the hot allocation the review flagged. (`eval/mod.rs` uses the same
/// `SymbolMap` pattern on its own loop.)
#[derive(Clone, Copy)]
enum SpecialHead {
    /// `quote` / `quasiquote` / `try` / `error-of` / `assert-error` / `%try`
    /// — return without descending. Mirrors `guards::skips_body`.
    SkipBody,
    If,
    /// `let` / `let*` — sequential bind, no pre-binding.
    Let,
    /// `letrec` — pre-bind every name before walking RHSs (mutual recursion).
    Letrec,
    /// `fn` / `lambda` — open a fresh scope with the params bound.
    Fn,
    /// `def` — `name` is a binder, value is an expression.
    Def,
    /// `defn` / `defmacro` — same shape as `fn`/`lambda` plus a binder name.
    Defn,
}

static SPECIAL_HEAD: LazyLock<SymbolMap<SpecialHead>> = LazyLock::new(|| {
    use SpecialHead::*;
    [
        (kw::QUOTE, SkipBody),
        (kw::QUASIQUOTE, SkipBody),
        (kw::TRY, SkipBody),
        (kw::ERROR_OF, SkipBody),
        (kw::ASSERT_ERROR, SkipBody),
        (kw::TRY_PRIM, SkipBody),
        // `(comment …)` expands to `nil` — its body is never evaluated, so
        // checking it would flag names that intentionally don't resolve (a
        // sketched call, a snippet from another project). The whole point of the
        // form is to hold code that doesn't run.
        (kw::COMMENT, SkipBody),
        (kw::IF, If),
        (kw::LET, Let),
        (kw::LETREC, Letrec),
        (kw::FN, Fn),
        (kw::DEF, Def),
        (kw::DEFN, Defn),
        (kw::DEFMACRO, Defn),
    ]
    .into_iter()
    .map(|(n, k)| (value::intern(n), k))
    .collect()
});

/// Walk `form` recursively, adding to `ctx.file_globals` every name introduced
/// by a `(def name …)` or `(defmacro name …)` — at any depth, since Brood's
/// `def` always binds globally regardless of where it textually sits (a
/// `(when … (def x 1))` makes `x` a global when the `when` runs).
///
/// Recursion stops at forms whose body is data, not code (`quote` /
/// `quasiquote`) — a `(quote (def x …))` is a literal list, not a binder.
/// Doesn't recurse into a `fn`/`lambda` body either: a `def` *inside* a
/// closure body only fires when the closure is called, but since the body
/// runs later and Brood's `def` is global, the result is the same — we still
/// want the name in scope. So we *do* recurse there. The only thing we skip
/// is `quote`/`quasiquote`.
pub(super) fn collect_def_names(heap: &Heap, form: Value, ctx: &mut Ctx) {
    // Deep-form stack safety — same stacker remedy as check_into above.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_def_names_inner(heap, form, ctx)
    })
}

fn collect_def_names_inner(heap: &Heap, form: Value, ctx: &mut Ctx) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    // Lock-free `symbol_is` instead of allocating the head's spelling — the
    // walk visits every nested form, and only four comparisons are needed.
    if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
        return;
    }
    if value::symbol_is(head, kw::DEF) || value::symbol_is(head, kw::DEFMACRO) {
        if let Some(&Value::Sym(name)) = items.get(1) {
            // Tag a macro definition (so the walk treats its calls' arguments as
            // opaque syntax); a plain `def` is just a global. `defmacro` lowers to
            // `(def name (%make-macro …))` in the *expanded* tree, so detect the
            // value shape too — the bare `defmacro` head only survives on the
            // un-expanded fragment path.
            let is_macro_def = value::symbol_is(head, kw::DEFMACRO)
                || items.get(2).is_some_and(|&v| is_make_macro_form(heap, v));
            if is_macro_def {
                ctx.add_file_macro(name);
            } else {
                ctx.add_file_global(name);
            }
            // This file defines `name`, so it's not an external dependency — mark it
            // own so the Phase-2 dep-keys exclude it (self-deps ride the file's mtime).
            heap.rec_check_dep_own(name);
            // If the value is a variadic `fn` (a `&` rest param), record it so a
            // later fixed-arity `(sig …)` declaration isn't misread as an exact
            // arity for a variadic callee (a false positive). A sig that itself
            // declares a `&` rest type is fine — it yields `Arity::at_least`.
            if items
                .get(2)
                .is_some_and(|&v| def_value_is_variadic(heap, v))
            {
                ctx.mark_variadic_global(name);
            }
        }
    } else if ctx.is_file_macro(head) {
        // A call to a file-local macro the checker can't expand (single-file mode,
        // or a macro defined in a deferred `test` thunk). A bare-symbol argument may
        // be a name the macro *defines* — `(pm-def-fac pm-qfac)` → `pm-qfac` — so
        // record those as file-globals; a later reference then isn't flagged
        // unbound. Sound: this only widens the bound set, never adds a warning. The
        // macro's source order puts its `defmacro` before this use, so it's already
        // in `file_macros` by now.
        for &arg in &items[1..] {
            if let Value::Sym(s) = arg {
                ctx.add_file_global(s);
            }
        }
    }
    for &item in &items[1..] {
        collect_def_names(heap, item, ctx);
    }
}

/// Is `form` a `(%make-macro …)` combination — the value a `defmacro` lowers to
/// once expanded? Recognises a file-local macro definition in the expanded tree
/// (where the `defmacro` head is gone, replaced by `(def name (%make-macro …))`).
fn is_make_macro_form(heap: &Heap, form: Value) -> bool {
    matches!(list_items(heap, form).as_deref(),
        Some([Value::Sym(h), ..]) if value::symbol_is(*h, "%make-macro"))
}

/// Does the value form of a `def` resolve to a **variadic** `fn`/`lambda` — one
/// whose parameter list (in any arm of a multi-arity fn) contains a `&` rest
/// marker? Reads the `(def name (fn …))` shape `defn` expands to; `false` for a
/// non-`fn` value or a fixed-arity one.
fn def_value_is_variadic(heap: &Heap, value_form: Value) -> bool {
    let Some(items) = fn_form_items(heap, value_form) else {
        return false;
    };
    // items = [fn, params-or-arm, body…]. A multi-arity fn has clause *lists*
    // (`((a) …) ((a & b) …)`); a single-arity fn has the param list directly.
    let rest = &items[1..];
    let rest = match rest.first() {
        // Peel a leading docstring for the single-arity shape.
        Some(Value::Str(_)) if rest.len() > 1 => &rest[1..],
        _ => rest,
    };
    rest.iter().any(|&part| part_has_rest(heap, part))
}

/// True if `part` — either a single-arity parameter list (`(a & b)`) or a
/// multi-arity clause (`((a & b) body…)`) — introduces a `&` rest parameter.
/// Checks the form as a param list, and if its first element is itself a list
/// (the clause shape), checks that nested param list too.
fn part_has_rest(heap: &Heap, part: Value) -> bool {
    if params_have_rest(heap, part) {
        return true;
    }
    // Multi-arity clause: ((params) body…) — look at the inner param list.
    match list_items(heap, part) {
        Some(items) => items
            .first()
            .is_some_and(|&inner| params_have_rest(heap, inner)),
        None => false,
    }
}

/// True if the parameter-list form `params` contains a `&` (or `&rest`) marker —
/// i.e. the function it belongs to is variadic. A vector or list param list is
/// accepted; a non-list form (e.g. a docstring) yields `false`.
fn params_have_rest(heap: &Heap, params: Value) -> bool {
    let items = match params {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => match list_items(heap, params) {
            Some(v) => v,
            None => return false,
        },
        _ => return false,
    };
    items.iter().any(|p| {
        matches!(p, &Value::Sym(s)
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST))
    })
}

pub(super) fn check_into(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    // The walk recurses per nesting level of the checked form, and a
    // deeply-nested-but-legal form (the kernel's deep-value tests build
    // 60k-deep lists) would blow the native stack. Grow it in heap-backed
    // segments instead (host-panic hardening — the same stacker remedy as
    // the kernel's deep-value walkers); unlike a depth cap this still CHECKS
    // the deep form, and termination is structural (immutable data has no
    // cycles).
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        check_into_inner(heap, form, ctx, out)
    })
}

fn check_into_inner(heap: &Heap, form: Value, ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&head) = items.first() else { return };

    // `(%lint-allow :category body…)` — the expansion of the `check-allow`
    // suppression macro. A runtime no-op (it just yields its body), but here it
    // adds `:category`'s lint to the suppressed set for the wrapped subtree, so a
    // deliberately-lint-tripping form (a non-tail-recursive JIT torture fn, a
    // redundant `match` clause under test) can silence exactly that lint without
    // a comment (the reader strips those before the checker runs). We still walk
    // the body for every *other* lint.
    if let Value::Sym(s) = head {
        if value::symbol_is(s, "%lint-allow") {
            let mask = lint_allow_mask(items.get(1).copied());
            let inner = ctx.with_suppressed(mask);
            for &arg in &items[1..] {
                check_into(heap, arg, &inner, out);
            }
            return;
        }
    }

    // **Ability op on a record-typed variable with no impl** (Slice 3, inference hook).
    // The syntactic pass in `protocol` already covers literal / direct-ctor args; this
    // uses the inferred type of a *symbol* argument (a `let`-bound record, a sig-typed
    // param) — so `(let (c (circle 2)) (size c))` is flagged when `Size` has no impl for
    // circle. Gated: file has abilities, head is a known op fn, arg is a symbol.
    if let (Some(info), Value::Sym(h)) = (ctx.ability(), head) {
        if let Some((ability, op)) = info.op_of(h) {
            if let Some(&Value::Sym(_)) = items.get(1) {
                if let Some(ty) = super::infer::expr_ty(heap, items[1], ctx) {
                    super::protocol::check_ability_call_inferred(
                        info,
                        h,
                        &ty,
                        heap.form_pos_only(form),
                        out,
                    );
                }
            }
            // **Typed op params** (ADR-180): check each argument against the op's declared
            // `(name T)` parameter type — the argument-side sibling of the `:-> RET` flow.
            // Same gradual relation as the sig-param check (`gradual_of` + `consistent_with`
            // + `relax_param_for_arg`), so it is false-positive-clean: a precise arg is
            // checked `⊆`, a dynamic arg `∩ ≠ ⊥`, and an unknown/NEVER arg defers. Param `i`
            // corresponds to argument `items[i + 1]` (`self` is param 0 / `items[1]`).
            if let Some(params) = info.op_params_of(h) {
                for (i, pty) in params.iter().enumerate() {
                    let Some(pty) = pty else { continue };
                    let Some(&arg) = items.get(i + 1) else { break };
                    let g = gradual_of(heap, arg, ctx);
                    if !g.bound.is_never()
                        && !g.clone().consistent_with(relax_param_for_arg(pty))
                        && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                    {
                        out.push((
                            arg_pos(heap, arg, form),
                            format!(
                                "ability {}/{}: argument {} expects {}, got {} ({})",
                                ability,
                                op,
                                i + 1,
                                pty,
                                g.bound,
                                crate::syntax::printer::print(heap, arg),
                            ),
                        ));
                    }
                }
            }
        }
    }

    // **Multimethod generic call whose args' identities come (partly) from inference** — the
    // `defmulti` analogue of the ability hook above (ADR-179). Fires when a `defmulti` generic
    // is applied with at least one symbol arg (so the syntactic pass in `protocol` deferred);
    // resolves each arg's identity syntactically or from its inferred record type.
    if let (Some(info), Value::Sym(h)) = (ctx.multi(), head) {
        if info.generic_of(h).is_some() {
            super::protocol::check_multi_call_inferred(
                heap,
                h,
                &items,
                info,
                ctx,
                heap.form_pos_only(form),
                out,
            );
        }
        // **Operator sugar on a record operand** (ADR-179): `(+ (usd 1) 2.5)` / `(< money 5)`
        // route to `num-*`/`compare-to`; warn when the routed multimethod has no method for the
        // pair. A record operand is required, so pure `(+ 1 2)` / `(< 1 2)` is never touched.
        super::protocol::check_operator_sugar(
            heap,
            h,
            &items,
            info,
            ctx,
            heap.form_pos_only(form),
            out,
        );
    }

    // **Keyword accessor** `(:key coll [default])` (ADR-165). A keyword head is not a
    // `Sym`, so none of the sig/arity machinery below sees it — the form was entirely
    // unchecked, including the misuse ADR-165 itself calls the most likely: `(:name
    // deps)` where `deps` is a *list* of maps. Two checks, both false-positive-free:
    // the arity, and whether the receiver's type can possibly be keyed.
    if let Value::Keyword(k) = head {
        let shown = format!(":{}", value::symbol_name_ref(k));
        let argc = items.len() - 1;
        if argc == 0 || argc > 2 {
            out.push((
                heap.form_pos_only(form),
                format!("{shown}: a keyword accessor takes 1 or 2 arguments, got {argc}"),
            ));
        } else {
            // The receivers `apply_keyword` accepts: a map (by key), a set (by
            // membership), or nil (empty). Warn only when the argument's type is
            // *provably* none of those — `is_disjoint` against the dynamic reading, so
            // an inferred/redefinable value never misfires.
            use crate::types::Tag;
            let keyed = Ty::of(Tag::Map)
                .union(Ty::of(Tag::Set))
                .union(Ty::of(Tag::Nil));
            let g = gradual_of(heap, items[1], ctx);
            if !g.bound.is_never()
                && g.bound.is_disjoint(&keyed)
                && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
            {
                out.push((
                    arg_pos(heap, items[1], form),
                    format!(
                        "{shown}: expected a map, set or nil to look up in, got {} ({})",
                        g.bound,
                        crate::syntax::printer::print(heap, items[1]),
                    ),
                ));
            }
        }
        for &arg in &items[1..] {
            check_into(heap, arg, ctx, out);
        }
        return;
    }

    // **`(get recv :literal-keyword …)` on an integer-indexed receiver** — the `get`
    // spelling of the check above, and the write-time half of ADR-164's runtime error.
    // A keyword key can only address something keyed, so a vector/list/string/bytes
    // receiver is a provable mistake (`(get deps :name)` where `deps` is a *list* of
    // maps). The curated signature can't express this: it constrains each argument
    // independently, and `countable` legitimately includes both keyed and indexed
    // kinds — the conflict is in the *relationship* between the two arguments.
    // Literal-keyword keys only, so a computed key is never guessed at.
    if let Value::Sym(s) = head {
        if value::symbol_is(s, "get") && items.len() >= 3 {
            if let Value::Keyword(_) = items[2] {
                use crate::types::Tag;
                let keyed = Ty::of(Tag::Map)
                    .union(Ty::of(Tag::Set))
                    .union(Ty::of(Tag::Nil));
                let g = gradual_of(heap, items[1], ctx);
                if !g.bound.is_never()
                    && g.bound.is_disjoint(&keyed)
                    && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                {
                    out.push((
                        arg_pos(heap, items[1], form),
                        format!(
                            "get: a keyword key needs a map, set or nil, got {} ({}) — \
                             an integer-indexed collection is indexed by position",
                            g.bound,
                            crate::syntax::printer::print(heap, items[1]),
                        ),
                    ));
                }
            }
        }
    }

    // Special-cased forms that introduce scope or refine types. Each handles
    // its own argument-walking and returns; the generic path below doesn't run.
    if let Value::Sym(s) = head {
        // One `SymbolMap` probe dispatches the recognised special-form heads —
        // no `value::symbol_name` allocation for the common short-circuit
        // paths (`if`/`let`/`fn`/…). The fallthrough computes the spelling
        // once for the call-resolution work below (sig/arity/error messages).
        if let Some(&kind) = SPECIAL_HEAD.get(&s) {
            match kind {
                SpecialHead::SkipBody => return,
                SpecialHead::If => {
                    check_if(heap, form, &items, ctx, out);
                    return;
                }
                SpecialHead::Let => {
                    check_let(heap, form, &items, ctx, out, false);
                    return;
                }
                SpecialHead::Letrec => {
                    // `letrec` pre-binds every name to `nil` so all bindings are
                    // visible in every RHS — that's the mutual-recursion reason
                    // letrec exists. The checker mirrors this: it pre-binds the
                    // names into the inner scope *before* walking the RHSs, so a
                    // self-recursive or mutually-recursive call doesn't get
                    // flagged unbound.
                    check_let(heap, form, &items, ctx, out, true);
                    return;
                }
                SpecialHead::Fn => {
                    check_fn(heap, &items, ctx, out);
                    return;
                }
                SpecialHead::Def => {
                    check_def(heap, form, &items, ctx, out);
                    return;
                }
                SpecialHead::Defn => {
                    check_defn(heap, &items, ctx, out);
                    return;
                }
            }
        }
        // Resolve the callee's signature + arity (separate concerns; either
        // may be available without the other). Both take `Symbol` directly —
        // no `symbol_name` round-trip — so the success path doesn't allocate.
        // A user `(sig …)` declaration wins over primitive/curated/inferred sigs
        // (it's the author's stated contract). For arity, the real callable's
        // arity stays authoritative; the declared param count only fills in when
        // the callee can't be inspected (a file-local `defn` in --check mode).
        let declared = if ctx.is_lexical_local(s) {
            None // a fn/let local shadows the name → not the declared global
        } else {
            ctx.declared_sig(s)
        };
        // A name this file `def`s/`defn`s supersedes whatever the image currently
        // binds (the file is checked *before* it loads, so a heap binding — a
        // builtin like `check`, a prelude closure — is by definition the OLD
        // value; ADR-123: a def always wins). Only the file-local declared sig
        // may describe it; never the stale heap signature.
        // `is_lexical_local` guards the heap fallback too: a shadowing local is not
        // the global, so its arg/return types are unknown — never the primitive's.
        let sig = declared.clone().or_else(|| {
            (!ctx.is_lexical_local(s) && !ctx.is_file_global(s))
                .then(|| sig_of(heap, s))
                .flatten()
        });
        // The real callable's arity is authoritative when known (a `sig!` wrapper
        // preserves the wrapped fn's arity); fall back to the declared param count
        // for a file-local defn the read-only checker can't inspect. A declared
        // sig with a `&` rest type uses `Arity::at_least`; `&optional` params widen
        // a fixed sig to a range instead of an exact count; a fixed-arity sig that
        // applies to a known-variadic global is suppressed (the sig's fixed count
        // is an undercount, so using it as an exact arity would be a false positive).
        //
        // A **lexical local shadows the global** — a `let`/`fn` binding named like a
        // builtin (`(let (exit (get o :exit)) (exit model))`) is the local, not the
        // primitive, so its arity is unknown here. Skip the whole computation, exactly
        // as the declared-sig lookup above does (`is_lexical_local` → `None`);
        // otherwise `arity_of` reads the global's arity and false-flags the call.
        let arity = if ctx.is_lexical_local(s) {
            None
        } else {
            (!ctx.is_file_global(s))
                .then(|| arity_of(heap, s))
                .flatten()
                .or_else(|| {
                    declared
                        .filter(|sg| sg.rest.is_some() || !ctx.is_variadic_global(s))
                        .map(|sg| {
                            if sg.rest.is_some() {
                                Arity::at_least(sg.params.len())
                            } else if sg.optional.is_empty() {
                                Arity::exact(sg.params.len())
                            } else {
                                Arity::range(sg.params.len(), sg.params.len() + sg.optional.len())
                            }
                        })
                })
        };
        // Unbound-symbol diagnostic: warn only when the head is **truly not
        // resolvable** — not local, not a syntactic keyword, not in the global
        // env (which includes `Value::Macro`s like `test` / `assert=` that
        // `arity_of` doesn't describe), and not in the curated stdlib table.
        // The unbound check is independent of "is the sig informative" —
        // a macro is bound even though it has no value-type sig.
        //
        // `is_syntactic_keyword` is the one piece that still wants the
        // spelling — but only when every other short-circuit has failed.
        // Compute it lazily.
        if is_unbound(heap, ctx, s) && !ctx.is_suppressed(super::ctx::SUPPRESS_UNBOUND) {
            out.push((heap.form_pos_only(form), unbound_msg(&name_of(s))));
            // Still recurse into args below — they may carry their own issues.
        } else if !ctx.is_suppressed(super::ctx::SUPPRESS_UNREQUIRED) {
            if let Some(m) = unrequired_module(heap, ctx, s) {
                out.push((heap.form_pos_only(form), unrequired_msg(&m)));
            }
        }

        // Operand-position unbound symbols. When the head evaluates its arguments
        // (primitive / known closure / lexical local — never a macro), a bare
        // symbol operand is a value reference, so an unresolvable one is genuinely
        // unbound. Gated by `evaluates_args` so an unexpanded macro argument or a
        // forward reference under an unknown head is never mistaken for one. The
        // bottom recursion walks Pair operands; this only adds the leaf case (a
        // bare `Sym`, which `check_into` itself skips), so no double-reporting.
        if evaluates_args(heap, ctx, s) {
            for &arg in &items[1..] {
                check_value_leaf(heap, arg, form, ctx, out);
            }
        }

        // Arity check (independent of sig — they're separate concerns).
        if let Some(a) = arity {
            let argc = items.len() - 1;
            if !a.accepts(argc) {
                out.push((
                    heap.form_pos_only(form),
                    format!(
                        "{}: wrong number of arguments — expected {}, got {}",
                        name_of(s),
                        arity_str(a),
                        argc,
                    ),
                ));
            }
        }

        // **Function-as-value lint** (advisory). A bare reference to a known
        // zero-arity global passed to an output sink (`print`/`println`/`str`/
        // `format`) is almost always a missing call: it stringifies the function
        // itself (`#<fn name>`) instead of its result. The classic
        // `(print ansi-clear)`-for-`(print (ansi-clear))` slip — otherwise
        // silent (it's legal, types fine, and runs). Restricted to the sinks and
        // to *globals* (a same-named local is left alone — `arity_of` only reads
        // the global env, but `is_local` keeps a shadowing binding quiet) so it
        // stays false-positive-free, per the checker's "rather miss than
        // misfire" rule. Only zero-arity is flagged: a fn that takes args is a
        // plausible intentional callback value.
        if is_output_sink(s) {
            for &arg in &items[1..] {
                if let Value::Sym(a) = arg {
                    if !ctx.is_local(a)
                        && !ctx.is_file_global(a) // a file redefinition supersedes the heap's arity
                        && matches!(arity_of(heap, a), Some(ar) if ar.min == 0 && ar.max == Some(0))
                    {
                        let n = name_of(a);
                        out.push((
                            heap.form_pos_only(form),
                            format!(
                                "{n}: function used as a value — did you mean ({n})? \
                                 the bare zero-arg function stringifies as #<fn {n}>, not its result"
                            ),
                        ));
                    }
                }
            }
        }

        // **Match-exhaustiveness lint** (ADR-118). `match` compiles a no-catch-
        // all form's failure to `(throw [:match-error 'context target
        // 'patterns])` — recognising that exact shape here (the generic
        // `throw` call path, not a dedicated `match`/`SPECIAL_HEAD` entry,
        // since by now `match` has already macroexpanded to this) is enough
        // to flag a literal-enum scrutinee whose clauses don't cover every
        // member. See `match_exhaustiveness_gap`.
        if value::symbol_is(s, "throw") && items.len() == 2 {
            if let Some(msg) = match_exhaustiveness_gap(heap, items[1], ctx) {
                out.push((heap.form_pos_only(form), msg));
            }
        }

        if let Some(sig) = sig {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Check the argument against the parameter with the **full gradual
                // relation** — the same `gradual_of` / `consistent_with` the
                // return-type check uses (ADR-110; gating "B1", docs/type-gating.md).
                //   - a **precise** argument (a literal singleton — B0 makes these
                //     faithful, a `(sig …)`-typed param, integer-closed arithmetic)
                //     is checked with `⊆`, catching a *merely-wider* misuse (a
                //     `number` where `int` is wanted) — closing the return/argument
                //     asymmetry;
                //   - a **dynamic** argument (a call result, an inferred/redefinable
                //     global) is checked with `∩ ≠ ⊥` (`!is_disjoint`), identical to
                //     the old behaviour — no new over-warning, reload-safe.
                // A `NEVER` bound means "this branch is unreachable" (a guard
                // narrowed the arg to the empty type); skip it — the code can't run,
                // so there's no real misuse to flag (the old `is_never` skip; under
                // the dynamic reading a bare NEVER would else read as
                // disjoint-from-everything).
                let g = gradual_of(heap, arg, ctx);
                // Relax the parameter for the membership test in the two places the
                // lattice deliberately under-approximates (see `relax_param_for_arg`),
                // so the advisory check never misfires; the original `param` is still
                // what the message reports.
                let param_relaxed = relax_param_for_arg(&param);
                if !g.bound.is_never()
                    && !g.clone().consistent_with(param_relaxed)
                    && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                {
                    let msg = format!(
                        "{}: argument {} expects {}, got {} ({})",
                        name_of(s),
                        i + 1,
                        param,
                        g.bound,
                        crate::syntax::printer::print(heap, arg),
                    );
                    // Anchor at the offending ARGUMENT when it's a positioned
                    // sub-form (a nested call), else the call form.
                    out.push((arg_pos(heap, arg, form), msg));
                }

                // Callback-arity check (ADR-078 arrows): when the parameter is a
                // function arrow with a fixed arity — a higher-order combinator
                // (`map`/`filter`/`reduce`/`fold`) that calls its callback with a
                // known argument count — flag a callback that provably can't
                // accept that count. Conservative: only fires when the callback's
                // arity is *known* (a named global fn, or a simple single-clause
                // lambda literal); a local, variadic, or multi-clause callback is
                // skipped — no false positives.
                if let Some(expected) = param.as_arrow() {
                    if expected.rest.is_none() {
                        let wanted = expected.params.len();
                        if let Some(cb) = callback_arity(heap, arg, ctx) {
                            if !cb.accepts(wanted) {
                                let msg = format!(
                                    "{}: argument {} is a callback called with {} \
                                     argument{}, but {} takes {}",
                                    name_of(s),
                                    i + 1,
                                    wanted,
                                    if wanted == 1 { "" } else { "s" },
                                    callback_desc(arg),
                                    arity_str(cb),
                                );
                                out.push((arg_pos(heap, arg, form), msg));
                            }
                        }
                    }
                }
            }
        }

        // **Overload argument check** (ADR-116 completion). A callee with a
        // declared overload (`(sig f (and (int -> int) (bool -> bool)))`) has no
        // single `sig` — its arms live in `declared_overload` — so the per-arg
        // loop above skipped it. Flag a call whose arguments match *no* arm.
        // Sound by construction (see `overload_arg_mismatch`): disjointness, not
        // subtyping, and only when every arity-relevant arm is ruled out.
        if !ctx.is_lexical_local(s) {
            if let Some(arms) = ctx
                .declared_overload(s)
                .cloned()
                .or_else(|| declared_heap_overload(heap, s))
            {
                let arg_tys: Vec<Option<Ty>> =
                    items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                if overload_arg_mismatch(&arms, &arg_tys) {
                    out.push((
                        heap.form_pos_only(form),
                        format!("{}: no overload clause accepts these arguments", name_of(s)),
                    ));
                }
            }
        }
    }

    // Recurse into arguments (and nested forms) — unless the head is an
    // unexpandable macro, whose operands are opaque syntax, not evaluated code
    // (see `resolves_to_macro`). Walking a macro's args as code would false-flag a
    // template's spliced binders (`(wp v (+ a b))` where `wp` binds `a`/`b`).
    let head_is_macro =
        matches!(items.first(), Some(&Value::Sym(s)) if resolves_to_macro(heap, ctx, s));
    if !head_is_macro {
        for &item in &items {
            check_into(heap, item, ctx, out);
        }
    }
}

/// `(fn (params...) docstring? body...)` (and `lambda` — the same closure
/// shape) — parse the parameter list, bind each into `ctx`, then walk the body
/// in the extended scope. Parameter positions (`& rest`, `&optional`) are
/// binders, not references, so they're not flagged as unbound.
fn check_fn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    check_fn_seeded(heap, items, ctx, out, None, None);
}

/// `check_fn`, optionally seeding the parameters from a `(sig …)` signature — used
/// when this `fn` is the value of a `(def name …)` whose `name` is declared. Each
/// parameter is then bound to its declared type *and* marked a sig-typed param,
/// so the body's checks know the types and a guard narrowing a parameter to the
/// empty type surfaces as a dead clause (`check_if`). Seeds only on an exact
/// positional match (no rest, equal arity) so positions can't misalign.
fn check_fn_seeded(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    sig: Option<&crate::types::Sig>,
    name: Option<Symbol>,
) {
    // Multi-arity `fn` — `(fn ((a) …) ((a b) …))` — isn't one param list + body;
    // each form (after an optional docstring) is a clause `(param-list body…)`.
    // Bind *every* clause's params into one scope and walk every body. Over-binding
    // (a param from clause N visible in clause M's body) only widens scope, so it
    // can never manufacture a false positive — it just stops a param used in one
    // clause from looking unbound. The sig seeding (single positional match) doesn't
    // apply to a multi-arity callee, so it's dropped here.
    if crate::eval::macros::fn_is_arity_multi_clause(heap, items) {
        let forms = &items[1..];
        let forms = match forms.first() {
            Some(Value::Str(_)) if forms.len() > 1 => &forms[1..],
            _ => forms,
        };
        let mut scope = ctx.clone();
        for &clause in forms {
            if let Some(citems) = list_items(heap, clause) {
                if let Some(&plist) = citems.first() {
                    for p in fn_params(heap, plist) {
                        scope = scope.bind(p, None);
                    }
                }
            }
        }
        for &clause in forms {
            if let Some(citems) = list_items(heap, clause) {
                for &body_form in citems.get(1..).unwrap_or(&[]) {
                    check_into(heap, body_form, &scope, out);
                }
            }
        }
        return;
    }
    let Some(&params_form) = items.get(1) else {
        return;
    };
    let params = fn_params(heap, params_form);
    // Whether the param list ends in a `& rest` binder (always the last binder). Its
    // seeded type differs — the binder collects the variadic args into a *list*.
    let has_rest = params_form_has_rest(heap, params_form);
    // The closure's actual param count must fall inside the declared sig's
    // arity range for seeding to make sense: at least `params.len()`
    // required, at most `params.len() + optional.len()` unless it has a
    // rest tail (any count at or above `params.len()` is then fine).
    let sig = sig.filter(|s| {
        params.len() >= s.params.len()
            && (s.rest.is_some() || params.len() <= s.params.len() + s.optional.len())
    });
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        // An `&optional` position may genuinely be absent at the call site
        // (bound to `nil`, same as an unsupplied optional with no default) —
        // widen with `nil` and seed it as a plain (not sig-authoritative)
        // type, so a defensive `(nil? p)` in the body is never mistaken for
        // dead code the way an exact required-param contract would be.
        // The `& rest` binder (always last) collects the variadic arguments into a
        // list, so its type is `list<rest-elem>` — not the element type the sig's
        // rest position carries. Seeding it as the bare element type was a false-
        // positive source: `(defn f (& xs) (reduce + 0 xs))` with `(sig f (& int ->
        // …))` would type `xs` as `int` and then flag `(reduce … xs)` for passing an
        // int where a sequence is wanted. Bind it plainly (not sig-authoritative) so
        // no dead-clause lint keys off it.
        if has_rest && i + 1 == params.len() {
            let rest_ty = sig.and_then(|s| s.rest.clone()).map(Ty::list_of);
            scope = scope.bind(p, rest_ty);
            continue;
        }
        let is_optional_pos =
            sig.is_some_and(|s| i >= s.params.len() && i < s.params.len() + s.optional.len());
        match sig.and_then(|s| s.param(i)) {
            Some(ty) if is_optional_pos => {
                scope = scope.bind(p, Some(ty.union(Ty::of(crate::core::value::Tag::Nil))));
            }
            Some(ty) => scope = scope.bind_sig_param(p, ty),
            None => scope = scope.bind(p, None),
        }
    }
    // Skip a leading docstring (a lone string when more body follows).
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
    // Return-type check (a `GradualTy` consumer): the body's last form is the
    // function's result, which must be *consistent* with the declared return `R`.
    // `gradual_of` makes an over-approximated result (a call) `dynamic`, so the ∩
    // relation only warns on a body type provably disjoint from `R` — never on a
    // widened guess (a `number`-returning body declared `int` defers). A precise
    // literal return uses `⊆`. Only fires with a seeded (sig-matched) sig.
    if let Some(s) = sig {
        if let Some(&ret_form) = items[body_start..].last() {
            let g = gradual_of(heap, ret_form, &scope);
            if !g.consistent_with(s.ret.clone())
                && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
            {
                let who = name
                    .map(|n| format!("{}: ", name_of(n)))
                    .unwrap_or_default();
                out.push((
                    heap.form_pos_only(ret_form),
                    format!(
                        "{}declared return type {} but the body yields {}",
                        who, s.ret, g.bound
                    ),
                ));
            }
        }
    }
}

/// Impl-return conformance: walk the expanded tree for each `(register-impl 'A 'op :id
/// (fn params body…) 'ns)` and, when ability `A`'s op `op` declares a `:-> RET` return
/// type, check the impl's body against it. The gradual relation keeps this
/// false-positive-clean the same way the `sig` return check does — an over-approximated
/// body (a call result) is `dynamic` and defers; only a body *provably disjoint* from the
/// declared return warns. Called after `ctx.set_ability(…)`, so `ctx.ability()` is live.
pub(super) fn check_impl_returns(
    heap: &Heap,
    expanded: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if ctx.ability().is_none_or(|i| i.is_empty()) {
        return;
    }
    for &form in expanded {
        walk_impl_returns(heap, form, ctx, out);
    }
}

fn walk_impl_returns(heap: &Heap, form: Value, ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(head)) = items.first() {
            if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
                return;
            }
            if head_name(head) == "register-impl" {
                check_one_impl_return(heap, &items, ctx, out);
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            walk_impl_returns(heap, item, ctx, out);
        }
    })
}

/// Check one `(register-impl 'A 'op :id (fn params body…) 'ns)` against its op's declared
/// return type. The ability + op names come from the two quoted args; the impl fn is
/// arg 4.
fn check_one_impl_return(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let info = match ctx.ability() {
        Some(i) => i,
        None => return,
    };
    let (Some(ability), Some(op)) = (
        items.get(1).and_then(|&v| quoted_sym_name(heap, v)),
        items.get(2).and_then(|&v| quoted_sym_name(heap, v)),
    ) else {
        return;
    };
    let Some(ret) = info.op_ret_by_name(&ability, &op) else {
        return;
    };
    // An `any` declared return imposes no constraint — nothing to check.
    if ret.is_any() {
        return;
    }
    let id = items.get(3).copied().and_then(|v| match v {
        Value::Keyword(k) => Some(value::symbol_name(k)),
        _ => None,
    });
    let Some(&fn_form) = items.get(4) else {
        return;
    };
    let Some(fn_items) = list_items(heap, fn_form) else {
        return;
    };
    if !matches!(fn_items.first(), Some(&Value::Sym(s)) if is_fn_head(s)) {
        return;
    }
    // Multi-clause impl fns aren't produced by `impl`; if one appears, don't try to
    // pin a single arity — skip.
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &fn_items) {
        return;
    }
    let Some(&params_form) = fn_items.get(1) else {
        return;
    };
    // Bind the impl's params, seeding each with the op spec's declared `(name T)` type where
    // present (a contract, bound authoritatively like a sig param so the body checks against
    // it) and unknown otherwise, then grade the body's last form against the declared return.
    let param_types = info.op_params_by_name(&ability, &op);
    let mut scope = ctx.clone();
    for (i, p) in fn_params(heap, params_form).into_iter().enumerate() {
        match param_types
            .and_then(|pts| pts.get(i))
            .and_then(Option::as_ref)
        {
            Some(ty) => scope = scope.bind_sig_param(p, ty.clone()),
            None => scope = scope.bind(p, None),
        }
    }
    let body_start = match (fn_items.get(2), fn_items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    let Some(&ret_form) = fn_items.get(body_start..).and_then(|b| b.last()) else {
        return;
    };
    let g = gradual_of(heap, ret_form, &scope);
    if !g.consistent_with(ret.clone()) && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH) {
        let id_str = id.map(|i| format!(" for :{i}")).unwrap_or_default();
        out.push((
            heap.form_pos_only(ret_form),
            format!(
                "ability {ability}/{op}{id_str}: declared return type {} but the impl yields {}",
                ret, g.bound
            ),
        ));
    }
}

/// `(quote SYM)` → the symbol's name; used to read `register-impl`'s quoted ability / op.
fn quoted_sym_name(heap: &Heap, v: Value) -> Option<String> {
    let items = list_items(heap, v)?;
    if items.len() == 2 && matches!(items[0], Value::Sym(s) if value::symbol_is(s, kw::QUOTE)) {
        match items[1] {
            Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
            _ => None,
        }
    } else {
        None
    }
}

/// The bare (last `/`-segment) name of a symbol head — `ability/register-impl` and a
/// bare `register-impl` both read as `"register-impl"`.
fn head_name(sym: Symbol) -> String {
    let full = value::symbol_name(sym);
    full.rsplit('/').next().unwrap_or(&full).to_string()
}

/// The items of `form` when it is an `(fn …)` form, else `None` —
/// so `check_def` can recognise the `(def name (fn …))` shape that `defn`
/// expands to.
fn fn_form_items(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    let items = list_items(heap, form)?;
    match items.first()? {
        &Value::Sym(s) if is_fn_head(s) => Some(items),
        _ => None,
    }
}

/// The **gradual** type of an expression in *assignment* position — the value
/// flowing into a `(def x …)` whose `x` has a declared value type. This is the
/// first consumer of [`GradualTy`] (ADR-024): the gradual `dynamic()` is what lets
/// the check defer on a redefinable reference instead of fighting hot reload.
///
/// - A **literal** (non-symbol, non-call) has an exact, non-redefinable type →
///   `stat(t)` (checked with `⊆` — sound because the type is precise).
/// - A bare reference to a **redefinable global** is `dynamic`, bounded by its own
///   declared value type when it has one (`dynamic_within(t)`) or pure `dynamic()`
///   otherwise — the *bounded-dynamic* case `Option<Ty>` can't represent, and what
///   lets `(def x g)` be caught when `g`'s declared type is disjoint from `x`'s.
/// - A **local** or a **call result** carries an *over-approximated* type, so it's
///   `dynamic_within(t)` — consistency then uses `∩ ≠ ⊥`, which can't over-warn on a
///   widened type (a number-returning call assigned to an `int` slot defers, not
///   warns). Unknown → pure `dynamic()` (always consistent — defer).
fn gradual_of(heap: &Heap, expr: Value, ctx: &Ctx) -> GradualTy {
    if let Value::Sym(s) = expr {
        // A known/narrowed type for `s` in the current scope — a fn param, a let
        // binding, OR a *guard narrowing* on any variable (a narrowing lands in
        // `ctx.get`, whether or not `s` is a lexical local, so this must be checked
        // for every symbol — gating it on `is_lexical_local` dropped a narrowing on
        // a free variable). A `(sig …)`-seeded param carries its *exact* contract
        // type → `stat` (precise, `⊆`): using it where a narrower type is wanted is
        // a real mismatch. Anything else (a `let` local whose RHS was a call, a
        // guard-narrowed variable) is an over-approximation bound → `dynamic_within`
        // (the `∩` relation, which never over-warns on a merely-wider type).
        if let Some(t) = ctx.get(s) {
            // `any` is the exception: a param declared `any` (e.g. `(sig set (any ->
            // set))` for "any seqable") carries *no* constraint — it is the gradual
            // "unknown", not a precise top type. Treating it as `stat(ANY)` would then
            // fail a `⊆` test against any narrower param (`(fold … coll)` wants a
            // collection), a false positive. So `any` is always `dynamic` — the
            // `dynamic()`-not-`Any` rule, applied to a declared param.
            return if ctx.is_sig_param(s) && !t.is_any() {
                GradualTy::stat(t)
            } else {
                GradualTy::dynamic_within(t)
            };
        }
        // A lexical local with no known type is in scope but unknown → `dynamic()`.
        if ctx.is_lexical_local(s) {
            return GradualTy::dynamic();
        }
        // Otherwise a (redefinable) global / file-global: dynamic, bounded by its
        // own declared value type when it has one — the bounded-dynamic case.
        // The file-local ctx (a `(sig …)` in *this* file's un-expanded forms)
        // wins; the heap-wide store (`declared_heap_value_ty`) covers a
        // same-module reference that got qualified to `mod/name` during
        // expansion, or a genuine cross-module reference — same fix
        // `declared_heap_sig` already applies for arrows.
        // For a name this file redefines, the heap-wide stores describe the OLD
        // binding (the file is checked pre-load) — only the file-local ctx
        // sources apply (ADR-123: a def always wins).
        let heap_declared = (!ctx.is_file_global(s))
            .then(|| declared_heap_value_ty(heap, s))
            .flatten();
        let heap_global = (!ctx.is_file_global(s))
            .then(|| global_value_ty(heap, s))
            .flatten();
        return match ctx
            .declared_value_ty(s)
            .or(heap_declared)
            .or_else(|| ctx.inferred_value_ty(s))
            .or(heap_global)
        {
            // The Gap A inferred current-image type (same-file `inferred_value_ty`,
            // or cross-file `global_value_ty` read from the loaded image) is exposed
            // as `dynamic_within` like a declared global — the `∩` relation, so a
            // reload that changes it is re-checked, never a stale hard proof.
            Some(t) => GradualTy::dynamic_within(t),
            None => GradualTy::dynamic(),
        };
    }
    // A compound form: control-flow forms recurse into their result positions
    // (each one's gradual type, joined), so a body assembled from *precise* pieces
    // (literals, sig-params, integer-closed arithmetic) is `stat` (checked `⊆`,
    // catching a merely-wider body), while any over-approximated call branch makes
    // the join `dynamic` (the ∩-relation, which never over-warns on a widened type).
    if matches!(expr, Value::Pair(_)) {
        if let Some(g) = gradual_of_compound(heap, expr, ctx) {
            return g;
        }
    }
    match expr_ty(heap, expr, ctx) {
        // A bare literal (not a call) is exact → static.
        Some(t) if !matches!(expr, Value::Pair(_)) => GradualTy::stat(t),
        // A call result is an over-approximation → dynamic (∩-relation, no over-warn).
        Some(t) => GradualTy::dynamic_within(t),
        None => GradualTy::dynamic(),
    }
}

/// The gradual type of a *compound* (`Pair`) expression when it's a form whose
/// result we can type **precisely** — a control-flow form (whose value is one of
/// its sub-forms) or the integer-closed arithmetic rule. Recurses into each result
/// position via [`gradual_of`], so the staticness propagates: an all-precise body
/// stays `stat` (warns on a merely-wider type via `⊆`), and any over-approximated
/// call branch makes the join `dynamic` (defers on widening). Returns `None` for
/// any other form (a plain call / unrecognised shape) — the caller then uses the
/// flat `expr_ty` → `dynamic_within` path. Mirrors `guards::control_flow_ty`'s shape
/// but carries the gradual `?` so the return/assignment check stays
/// false-positive-clean.
fn gradual_of_compound(heap: &Heap, expr: Value, ctx: &Ctx) -> Option<GradualTy> {
    let items = list_items(heap, expr)?;
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    // A lexical local can shadow a special-form name; then it isn't this form.
    if ctx.is_lexical_local(head) {
        return None;
    }
    // `(if test then else)` → join(then, else); `(if test then)` → then | nil.
    // Narrow each branch by what the test guard asserts, mirroring `check_if` —
    // so `(if (int? x) x 0)` types the then-branch's `x` as `int`, not the
    // declared `number` (the precise return-check would otherwise false-positive).
    if value::symbol_is(head, kw::IF) {
        let test = items.get(1).copied().unwrap_or(Value::nil());
        let (then_ctx, else_ctx) = match guard_assertion(heap, test, ctx) {
            Some(g) => {
                let then_ctx = ctx.narrow(g.sym, g.ty.clone());
                let else_ctx = if g.then_only {
                    ctx.clone()
                } else {
                    ctx.narrow(g.sym, g.ty.negate())
                };
                (then_ctx, else_ctx)
            }
            None => (ctx.clone(), ctx.clone()),
        };
        return match items.len() {
            4 => Some(
                gradual_of(heap, items[2], &then_ctx).union(gradual_of(heap, items[3], &else_ctx)),
            ),
            3 => Some(gradual_of(heap, items[2], &then_ctx).union(GradualTy::stat(NIL_TY))),
            _ => None,
        };
    }
    // `(do … last)` → gradual(last). Empty `(do)` → nil.
    if value::symbol_is(head, kw::DO) {
        return match items.last() {
            Some(&last) if items.len() > 1 => Some(gradual_of(heap, last, ctx)),
            _ => Some(GradualTy::stat(NIL_TY)),
        };
    }
    // `(when t body…)` / `(unless t body…)` → gradual(last) | nil.
    if value::symbol_is(head, kw::WHEN) || value::symbol_is(head, kw::UNLESS) {
        let &last = items.last()?;
        if items.len() < 3 {
            return Some(GradualTy::stat(NIL_TY));
        }
        return Some(gradual_of(heap, last, ctx).union(GradualTy::stat(NIL_TY)));
    }
    // `let`/`letrec` → gradual(last body), with each binding's RHS type
    // threaded into the scope (so a precise RHS makes its uses precise).
    if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
        let binds = bindings(heap, *items.get(1)?)?;
        if binds.len() % 2 != 0 || items.len() < 3 {
            return None;
        }
        let mut scope = ctx.clone();
        let mut i = 0;
        while i < binds.len() {
            let Value::Sym(name) = binds[i] else {
                return None; // destructuring binder — can't pin; defer the form
            };
            let rhs_ty = expr_ty(heap, binds[i + 1], &scope);
            scope = scope.bind(name, rhs_ty);
            i += 2;
        }
        let &last = items.last()?;
        return Some(gradual_of(heap, last, &scope));
    }
    // `(cond t1 r1 … :else rN)` → join of the result positions (odd index ≥ 2).
    if value::symbol_is(head, kw::COND) {
        let results: Vec<Value> = items[2..].iter().step_by(2).copied().collect();
        return gradual_join(heap, &results, ctx);
    }
    // `(case key v1 r1 … [default])` → join of each pair's result + a lone default.
    if value::symbol_is(head, kw::CASE) && items.len() >= 4 {
        let clauses = &items[2..];
        let mut results = Vec::new();
        let mut i = 0;
        while i < clauses.len() {
            if i + 1 < clauses.len() {
                results.push(clauses[i + 1]);
                i += 2;
            } else {
                results.push(clauses[i]);
                i += 1;
            }
        }
        return gradual_join(heap, &results, ctx);
    }
    // `(match scrut pat1 body1 …)` → join of the arm bodies (even offset from 3).
    if value::symbol_is(head, kw::MATCH) {
        let bodies: Vec<Value> = items[3..].iter().step_by(2).copied().collect();
        return gradual_join(heap, &bodies, ctx);
    }
    // `(and …)` / `(or …)` → join of operands (any can be the short-circuit value).
    if value::symbol_is(head, kw::AND) {
        if items.len() == 1 {
            return Some(GradualTy::stat(Ty::of(value::Tag::Bool)));
        }
        return gradual_join(heap, &items[1..], ctx);
    }
    if value::symbol_is(head, kw::OR) {
        if items.len() == 1 {
            return Some(GradualTy::stat(NIL_TY));
        }
        return gradual_join(heap, &items[1..], ctx);
    }
    // The integer-closed arithmetic rule produces a *precise* `int` (not an
    // over-approximation), so a `(* x x)`-style body declared `int` is `stat` and
    // checked with `⊆` — no false positive (the rule only fires when every operand
    // is a known integer; `guards::expr_ty` routes it through `numeric_call_ty`).
    if let Some(t) = expr_ty(heap, expr, ctx) {
        if is_int_closed_op(head) && t.is_subtype(&Ty::of(value::Tag::Int)) {
            return Some(GradualTy::stat(t));
        }
    }
    None
}

/// `nil` as a `Ty` — the value of an empty/else-less control-flow branch.
const NIL_TY: Ty = Ty::of(value::Tag::Nil);

/// Join the gradual types of several branch result forms (the `cond`/`case`/
/// `match`/`and`/`or` arms). `None` when there are no results (an empty clause
/// list — defer rather than invent a type).
fn gradual_join(heap: &Heap, forms: &[Value], ctx: &Ctx) -> Option<GradualTy> {
    let mut acc: Option<GradualTy> = None;
    for &f in forms {
        let g = gradual_of(heap, f, ctx);
        acc = Some(match acc {
            Some(a) => a.union(g),
            None => g,
        });
    }
    acc
}

/// Is `head` one of the integer-closed arithmetic ops whose result on integer
/// operands is precisely `int` (mirrors `guards::numeric_call_ty`)? `/` is excluded
/// (int/int can be a float).
fn is_int_closed_op(head: Symbol) -> bool {
    value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "quot")
        || value::symbol_is(head, "rem")
        || value::symbol_is(head, "mod")
        || value::symbol_is(head, "abs")
}

/// `(def name value)` — the binder is in position 1, the value in 2. Don't
/// flag `name` as an unbound *reference* (it's a binder); walk `value` as an
/// expression. `name` is added to the file-globals accumulator inside
/// [`check_file`], not here (which checks one form in isolation).
fn check_def(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(&value_form) = items.get(2) else {
        // `(def name)` — degenerate; skip.
        return;
    };
    // `(def name (fn …))` where `name` carries a `(sig …)` — the shape `defn`
    // expands to. Seed the fn's params with the declared types so the body knows
    // them (and a guard narrowing a param to `never` becomes a dead clause).
    if let Some(&Value::Sym(name)) = items.get(1) {
        // `ctx.declared_sig` is keyed by the *bare* name Pass 2.5 recorded from
        // the file's un-expanded `(sig …)` text; `name` here is `defn`'s
        // *expanded* def head, which is module-qualified inside a `defmodule`
        // block. The two only match at the root namespace — so a
        // `defmodule`-wrapped `(sig f …)` + `(defn f …)` pair needs the same
        // heap-wide fallback `gradual_of`'s reference branch already has
        // (ADR-124): `declared_heap_sig` reads the qualified store
        // `%register-sig` populates, so it matches regardless of namespace.
        let declared = ctx
            .declared_sig(name)
            .or_else(|| declared_heap_sig(heap, name));
        if let Some(sig) = declared {
            if let Some(fn_items) = fn_form_items(heap, value_form) {
                check_fn_seeded(heap, &fn_items, ctx, out, Some(&sig), Some(name));
                return;
            }
        }
        // Gradual-assignment check (the first `GradualTy` consumer): when `name`
        // carries a non-arrow `(sig name T)`, the assigned value must be
        // *consistent* with `T`. A dynamic value (a redefinable global, an
        // unknown) defers; a value whose type is provably incompatible with `T`
        // is flagged. Sound: `consistent_with` only rejects a provable mismatch
        // (`bound ∩ T = ⊥`, or a precise literal `⊄ T`), never a widened guess.
        let declared_value_ty = ctx
            .declared_value_ty(name)
            .or_else(|| declared_heap_value_ty(heap, name));
        if let Some(t) = declared_value_ty {
            let g = gradual_of(heap, value_form, ctx);
            if !g.consistent_with(t.clone()) {
                out.push((
                    heap.form_pos_only(form),
                    format!(
                        "{}: value of type {} is not assignable to declared type {}",
                        name_of(name),
                        g.bound,
                        t,
                    ),
                ));
            }
        }
    }
    // The value slot is evaluated — a bare unbound symbol there (`(def x typo)`)
    // is a reference error, same rule as a call operand.
    check_value_leaf(heap, value_form, form, ctx, out);
    check_into(heap, value_form, ctx, out);
}

/// `(defn name (params...) docstring? body...)` and the structurally identical
/// `defmacro` — the body lives in a fresh scope with `params` bound. Like
/// `def`, the `name` is a binder, not a reference; file-global accumulation
/// happens in [`check_file`].
fn check_defn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&params_form) = items.get(2) else {
        return;
    };
    // Un-expanded `defn` path (e.g. `(check 'form)` without expansion). Whole-file
    // checking expands `defn` to `(def name (fn …))` first, so a sig'd function's
    // params are actually seeded in `check_def`; here there's no declared sig to
    // consult, so just bind the params.
    let mut scope = ctx.clone();
    for p in fn_params(heap, params_form) {
        scope = scope.bind(p, None);
    }
    let body_start = match (items.get(3), items.get(4)) {
        (Some(Value::Str(_)), Some(_)) => 4,
        _ => 3,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// The set of parameter-binder symbols introduced by a `fn`/`defn`/`defmacro`
/// parameter list. Handles the three Brood shapes uniformly:
///
/// - positional: `(x y z)` → `{x, y, z}`
/// - optional:   `(x &optional (y 0))` → `{x, y}`
/// - rest:       `(x & ys)` → `{x, ys}`
///
/// `&` / `&optional` themselves are markers, not binders, so they're filtered
/// out. The result is *just* what would be in scope — used to seed `Ctx`
/// without false-flagging the inner body's references.
/// Does this parameter list end in a `& rest` tail? (The `&`/`&rest` marker; a
/// bare-symbol binder follows it.) Used to seed the rest binder as `list<elem>`.
fn params_form_has_rest(heap: &Heap, form: Value) -> bool {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return false,
    };
    items.iter().any(|&it| {
        matches!(it, Value::Sym(s) if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST))
    })
}

pub(super) fn fn_params(heap: &Heap, form: Value) -> Vec<Symbol> {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Sym(s) => {
                // Lock-free `symbol_is` to filter the parameter-list markers
                // — three name compares without ever allocating the spelling.
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST)
                {
                    continue;
                }
                out.push(s);
            }
            // `&optional` defaults: `(name default)` — the binder is at [0].
            Value::Pair(_) | Value::Vector(_) => {
                let inner = match item {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => list_items(heap, item).unwrap_or_default(),
                };
                if let Some(&Value::Sym(s)) = inner.first() {
                    out.push(s);
                }
            }
            _ => {}
        }
    }
    out
}

/// `(if test then else?)` — check the test in the outer ctx, then descend
/// into each branch with the ctx narrowed by what the test would assert.
/// `else` defaults to `nil` (matches the evaluator), so absent or non-pair
/// branches simply contribute no warnings.
fn check_if(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let test = items.get(1).copied().unwrap_or(Value::nil());
    let then_form = items.get(2).copied().unwrap_or(Value::nil());
    let else_form = items.get(3).copied().unwrap_or(Value::nil());

    // All three slots are evaluated value positions — a bare unbound symbol in
    // any (`(if typo …)`) is a reference error. then/else use the narrowed ctx,
    // matching how they're walked.
    check_value_leaf(heap, test, form, ctx, out);
    check_into(heap, test, ctx, out);

    // **Match-redundancy lint** (ADR-122). If this `if`'s test is itself a
    // literal `%eq` guard, scan forward through the `else`-chain for another
    // test of the same symbol against the same literal — the shape
    // `match`/`cond` compile duplicate clauses into (whichever occurs first
    // always wins, so a later one is dead code). Purely structural — no
    // scrutinee `Ty` involved, so this fires on any hand-written same-symbol
    // `%eq`-if chain too, not just `match`-generated ones.
    if !ctx.is_suppressed(super::ctx::SUPPRESS_UNREACHABLE) {
        if let Some((sym, lit)) = literal_eq_test_raw(heap, test) {
            if let Some(dup) = find_redundant_clause(heap, else_form, sym, lit) {
                let label =
                    render_literal_pattern(heap, lit).unwrap_or_else(|| "this value".to_string());
                out.push((
                    heap.form_pos_only(dup),
                    format!("match: unreachable clause — {label} is already handled above"),
                ));
            }
        }
    }

    let (then_ctx, else_ctx) = match guard_assertion(heap, test, ctx) {
        Some(g) => {
            let then_ctx = ctx.narrow(g.sym, g.ty.clone());
            // **Dead-clause lint.** If the guard narrowed a dead-clause-eligible
            // binding — a *sig-typed parameter* or a *precise surface `let`-local*
            // (ADR-131) — to the empty type, this branch can never run: the
            // binding's type is disjoint from what the guard (a `cond` predicate or
            // a `match` literal pattern, reached here via the scrutinee alias)
            // asserts. Eligibility (see `Ctx::dead_clause_locals`) is the whole of
            // the surface-vs-generated scoping: it admits only a non-gensym,
            // precisely-typed, immutable local, so a literal scrutinee, a
            // redefinable global, a call-result, and every macro-generated temp are
            // ruled out at the *binding*, never at the guard site — exactly how the
            // sig-param lint stays false-positive-free without inspecting positions.
            if let Some((p, known)) = then_ctx.newly_dead_binding(ctx) {
                out.push((
                    heap.form_pos_only(form),
                    format!(
                        "unreachable clause: {} is {}, which can never be {} \
                         — this branch is dead code",
                        name_of(p),
                        known,
                        g.ty,
                    ),
                ));
            }
            // Only narrow the else-branch when the guard is biconditional — a
            // `then_only` guard (the `and` short-circuit) doesn't establish `¬ty`
            // on a falsy test, so negating there would be a false positive.
            let else_ctx = if g.then_only {
                ctx.clone()
            } else {
                ctx.narrow(g.sym, g.ty.negate())
            };
            (then_ctx, else_ctx)
        }
        None => (ctx.clone(), ctx.clone()),
    };
    // Layer a **path** narrowing on top (occurrence typing through a `(get base
    // :key)` access): `(if (int? (get r :age)) …)` types `(get r :age)` as `int`
    // in the then-branch (and `¬int` in the else, for a biconditional predicate).
    let (then_ctx, else_ctx) = match path_guard_assertion(heap, test) {
        Some(pg) => {
            // Precise field-access narrowing for the exact path (both branches).
            let t = then_ctx.narrow_path(pg.base, pg.keys.clone(), pg.ty.clone());
            // Refine the *base*'s record type in the then-branch so the narrowing
            // flows when `base` is passed to a function (or otherwise used as a
            // value). Sound only in the then-branch: the guard being true proves
            // the whole access chain is present and typed. Only when every step is
            // a *field* — `base` is then an open record `{k1: {… {kn: ty}}}` (built
            // inner-out). A path with an *index* step would need a fixed-arity
            // tuple/vector refinement we can't infer from one position, so base
            // refinement is skipped there (the path narrowing above still applies).
            let all_fields: Option<Vec<_>> = pg
                .keys
                .iter()
                .map(|k| match k {
                    PathKey::Field(s) => Some(*s),
                    PathKey::Index(_) => None,
                })
                .collect();
            let t = match all_fields {
                Some(fields) => {
                    let base_record = fields.iter().rev().fold(pg.ty.clone(), |acc, &k| {
                        Ty::record_of(std::iter::once((k, (acc, true))).collect())
                    });
                    t.narrow(pg.base, base_record)
                }
                None => t,
            };
            let e = if pg.then_only {
                else_ctx
            } else {
                else_ctx.narrow_path(pg.base, pg.keys, pg.ty.negate())
            };
            (t, e)
        }
        None => (then_ctx, else_ctx),
    };
    check_value_leaf(heap, then_form, form, &then_ctx, out);
    check_into(heap, then_form, &then_ctx, out);
    check_value_leaf(heap, else_form, form, &else_ctx, out);
    check_into(heap, else_form, &else_ctx, out);
}

/// `(let bindings body…)` / `(letrec …)` — walk the bindings,
/// then check the body in the extended ctx. `letrec` pre-binds every name to
/// "in scope, type unknown" before walking RHSs, matching the evaluator's
/// nil-pre-bind so a self/mutual-recursive call inside a RHS isn't flagged
/// unbound. `let`/`let*` walk sequentially — each RHS sees only the
/// previously-bound names. (The let-vs-let* scope distinction doesn't affect
/// the unbound check since we only widen names; type-flow stays sound.)
///
/// Quietly skips a malformed bindings shape (a pattern-target `let`, an
/// improper list, an odd number of binding items): those are evaluator-level
/// errors and aren't this checker's job.
fn check_let(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    letrec: bool,
) {
    let Some(&binds_form) = items.get(1) else {
        return;
    };
    let Some(binds) = bindings(heap, binds_form) else {
        // Unknown shape — just recurse generically so we still check nested calls.
        for &item in items {
            check_into(heap, item, ctx, out);
        }
        return;
    };
    if binds.len() % 2 != 0 {
        return;
    }
    let mut scope = ctx.clone();
    // letrec: pre-bind every name to `None` (in scope, no known type) so each
    // RHS can refer to its peers (and to itself).
    if letrec {
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                scope = scope.bind(name, None);
            }
            j += 2;
        }
    } else {
        // Plain `let` is sequential, but a binding whose RHS is a `fn`/`lambda`
        // captures the let frame — the closure resolves its own binding name (and
        // its fn-valued siblings) by late lookup when *called*, so a self- or
        // mutually-recursive `let`-bound closure works at runtime. Pre-bind those
        // names so the unbound check agrees. Only fn-valued names, and only widening
        // scope, so an eager forward reference in a non-closure RHS still surfaces.
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                if fn_form_items(heap, binds[j + 1]).is_some() {
                    scope = scope.bind(name, None);
                }
            }
            j += 2;
        }
    }
    let mut i = 0;
    while i < binds.len() {
        let Value::Sym(name) = binds[i] else {
            // Destructuring binder (`(let ((a b) rhs) …)`): we can't pin a precise
            // type per position here, but the pattern's symbol leaves ARE bound in
            // the body — bind each to `None` (in scope, unknown type) so a use like
            // `(+ a b)` doesn't misfire as an unbound-symbol error. Still check the
            // RHS as an evaluated expression.
            check_value_leaf(heap, binds[i + 1], form, &scope, out);
            check_into(heap, binds[i + 1], &scope, out);
            for sym in pattern_syms(heap, binds[i]) {
                scope = scope.bind(sym, None);
            }
            i += 2;
            continue;
        };
        let rhs = binds[i + 1];
        // The RHS is an evaluated value position — a bare unbound symbol there
        // (`(let (x typo) …)`) is a reference error.
        check_value_leaf(heap, rhs, form, &scope, out);
        check_into(heap, rhs, &scope, out);
        let rhs_ty = expr_ty(heap, rhs, &scope);
        // Is the RHS *precise* (non-redefinable)? Computed in the pre-bind scope.
        // `dynamic == false` ⇔ a literal / integer-closed expression, never a
        // call-result or global reference — the reload-safe subset the dead-clause
        // lint may key off (ADR-131).
        let rhs_precise = !gradual_of(heap, rhs, &scope).dynamic;
        let rhs_guard = guard_assertion(heap, rhs, &scope);
        scope = scope.bind(name, rhs_ty.clone());
        // Dead-clause lint eligibility: a surface (non-gensym), precisely-typed
        // `let`-local joins the set the dead-clause lint may flag, so a later guard
        // that narrows it to `never` is caught — `(let (x 5) (cond (string? x) …))`.
        if rhs_precise
            && rhs_ty.as_ref().is_some_and(|t| !t.is_never())
            && !is_gensym_sym(name)
            && heap.form_pos_only(form).is_some()
        {
            scope.mark_dead_clause_local(name);
        }
        // Only alias a *biconditional* guard: a `then_only` guard (the `and`
        // short-circuit) must not be stored as a let-alias, or a later
        // `(if alias …)` would negate it in the else-branch (unsound).
        if let Some(g) = rhs_guard {
            if !g.then_only {
                scope = scope.add_guard(name, g.sym, g.ty);
            }
        }
        // A plain `(let (name other) …)` aliases `name` to `other` — narrowing
        // either propagates to the other via `narrow_chain`. This is what
        // makes the `match` pattern compiler's `(let (m__28 x) (if (%eq m__28
        // lit) …))` expansion narrow the user's `x`, not just the internal
        // `m__28`. We don't gate on `other` being a known local: it might be
        // a free reference (e.g. when checking a bare form via
        // `(check 'form)`) or a top-level global — either way, narrowing
        // inside the branch is sound (it describes "if this branch is
        // reached, then…", vacuously true on unreachable paths).
        if let Value::Sym(target) = rhs {
            scope = scope.add_alias(name, target);
        }
        i += 2;
    }
    for &body_form in &items[2..] {
        check_into(heap, body_form, &scope, out);
    }
    // Unused let binding lint. For each bound name, warn if it never appears
    // as a Value::Sym in any part of its visible scope: subsequent binding
    // elements + the body (plus preceding binding elements for letrec, where
    // any RHS may reference any other name). The scan is conservative — it
    // counts occurrences in binder positions and in quoted forms, so the only
    // errors are false negatives (missed warnings), never false positives.
    //
    // Exempt: names starting with `_` (the "intentionally unused" convention).
    {
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                let nm = name_of(name);
                // Exempt gensym temporaries (`<prefix>__<n>`, value::gensym): a
                // macro expansion (match / pattern lowering) can attach its
                // call-site position to the generated `let`, so the position
                // check below doesn't catch them — but the name does.
                let is_gensym = is_gensym_sym(name);
                // Exempt a binding that *shadows* an existing global or curated
                // builtin (`(let (list …) …)`, `(let (= …) …)`): you never
                // accidentally name a local after a builtin, so a shadow left
                // unused is a deliberate scope-isolation / hygiene test, not a
                // leftover. (`_`-prefixing can't express it — that changes the
                // name being shadowed.)
                let shadows_global = is_globally_bound(heap, name)
                    || curated_sig(name).is_some()
                    || ctx.is_file_global(name);
                if !nm.starts_with('_') && !is_gensym && !shadows_global {
                    // letrec: also scan preceding elements (mutual recursion).
                    let preceding_used =
                        letrec && binds[..j].iter().any(|&f| sym_appears_in(heap, f, name));
                    let following_used = binds[j + 2..]
                        .iter()
                        .any(|&f| sym_appears_in(heap, f, name));
                    let body_used = items[2..].iter().any(|&f| sym_appears_in(heap, f, name));
                    if !preceding_used && !following_used && !body_used {
                        // Only warn for user-written `let`s (those the reader
                        // assigned a source position). Compiler-generated lets
                        // (from match/pattern expansion) have no position and
                        // are exempt: their names are user-chosen but the
                        // "unused" status is an expansion artifact.
                        if let Some(pos) = heap.form_pos_only(form) {
                            out.push((Some(pos), format!("unused let binding: {}", nm)));
                        }
                    }
                }
            }
            j += 2;
        }
    }
}

/// The binder symbols of a destructuring pattern (`(a b)`, `[a b & rest]`,
/// nested `((a b) c)`) — every `Value::Sym` leaf except the `&` rest marker and
/// the `_` wildcard, which bind nothing. Literals (ints/keywords/strings) are
/// match constraints, not binders, so they're skipped. Used to put a pattern-let's
/// names in scope for the unbound-symbol check (a precise per-position type isn't
/// available, so each is bound to `None`).
fn pattern_syms(heap: &Heap, pat: Value) -> Vec<Symbol> {
    let mut out = Vec::new();
    collect_pattern_syms(heap, pat, &mut out);
    out
}

fn collect_pattern_syms(heap: &Heap, pat: Value, out: &mut Vec<Symbol>) {
    match pat {
        Value::Sym(s) => {
            let nm = name_of(s);
            if nm != "&" && nm != "_" {
                out.push(s);
            }
        }
        Value::Pair(_) | Value::Vector(_) => {
            if let Some(items) = bindings(heap, pat) {
                for it in items {
                    collect_pattern_syms(heap, it, out);
                }
            }
        }
        _ => {}
    }
}

/// Parse a `let` bindings form — accepts both `(name val name val …)` lists
/// and `[name val name val …]` vectors, the two shapes the reader emits.
fn bindings(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        Value::Nil | Value::Pair(_) => list_items(heap, form),
        _ => None,
    }
}

/// The elements of a proper list, or `None` for an improper list / non-list.
/// `pub(super)` because `sigs` (`infer_sig`) and `guards` (`guard_assertion`,
/// `expr_ty`) all need to peel a list head off a call form.
pub(super) fn list_items(heap: &Heap, mut v: Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    loop {
        match v {
            Value::Nil => return Some(out),
            Value::Pair(p) => {
                let (head, tail) = heap.pair(p);
                out.push(head);
                v = tail;
            }
            _ => return None,
        }
    }
}
