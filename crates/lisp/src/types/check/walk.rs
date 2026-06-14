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

use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::keywords as kw;
use crate::core::value::{self, Arity, Symbol, Value};
use crate::error::Pos;

use super::ctx::Ctx;
use super::guards::{expr_ty, guard_assertion, is_syntactic_keyword};
use super::sigs::{arity_of, arity_str, curated_sig, is_globally_bound, sig_of};

/// `symbol_name(s)` is a `String` allocation; we only need the spelling on
/// the rare *error* paths (unbound / arity / type-disjoint). Wrap as a
/// no-arg helper so the hot path (the whole `is_local` / `is_syntactic` /
/// `is_globally_bound` / `curated_sig` short-circuit) skips it entirely.
#[inline]
fn name_of(s: Symbol) -> String {
    value::symbol_name(s)
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
    value::symbol_is(head, kw::FN) || value::symbol_is(head, kw::LAMBDA)
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

/// How a callback argument reads in a diagnostic — a named function by its name,
/// an inline lambda as "the lambda".
fn callback_desc(arg: Value) -> String {
    match arg {
        Value::Sym(s) => name_of(s),
        _ => "the lambda".to_string(),
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
    if is_syntactic_keyword(&nm) {
        return false;
    }
    // A *qualified* reference (`mod/name`) whose module we don't know — no `mod/*`
    // is loaded — can't be proven unbound: the module may be defined dynamically
    // (`%load-string`, a required temp module) or live in a file a single-file
    // check didn't load. Stay silent. A typo in a *known* module (some `mod/*`
    // loaded) still falls through to the warning, so real qualified typos are kept.
    if let Some(slash) = nm.rfind('/') {
        if !ctx.module_is_known(&nm[..=slash]) {
            return false;
        }
    }
    true
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
    match heap.env_get(heap.global(), s) {
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
    ctx.is_file_macro(s) || matches!(heap.env_get(heap.global(), s), Some(Value::Macro(_)))
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
        if is_unbound(heap, ctx, s) {
            out.push((heap.form_pos(parent), unbound_msg(&name_of(s))));
        }
    }
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
        (kw::IF, If),
        (kw::LET, Let),
        (kw::LETREC, Letrec),
        (kw::FN, Fn),
        (kw::LAMBDA, Fn),
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
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&head) = items.first() else { return };

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
        let sig = declared.clone().or_else(|| sig_of(heap, s));
        // The real callable's arity is authoritative when known (a `sig!` wrapper
        // preserves the wrapped fn's arity); fall back to the declared param count
        // for a file-local defn the read-only checker can't inspect. A declared
        // sig with a `&` rest type uses `Arity::at_least`; a fixed-arity sig that
        // applies to a known-variadic global is suppressed (the sig's fixed count
        // is an undercount, so using it as an exact arity would be a false positive).
        let arity = arity_of(heap, s).or_else(|| {
            declared
                .filter(|sg| sg.rest.is_some() || !ctx.is_variadic_global(s))
                .map(|sg| {
                    if sg.rest.is_some() {
                        Arity::at_least(sg.params.len())
                    } else {
                        Arity::exact(sg.params.len())
                    }
                })
        });
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
        if is_unbound(heap, ctx, s) {
            out.push((heap.form_pos(form), unbound_msg(&name_of(s))));
            // Still recurse into args below — they may carry their own issues.
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
                    heap.form_pos(form),
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
                        && matches!(arity_of(heap, a), Some(ar) if ar.min == 0 && ar.max == Some(0))
                    {
                        let n = name_of(a);
                        out.push((
                            heap.form_pos(form),
                            format!(
                                "{n}: function used as a value — did you mean ({n})? \
                                 the bare zero-arg function stringifies as #<fn {n}>, not its result"
                            ),
                        ));
                    }
                }
            }
        }

        if let Some(sig) = sig {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Warn only on a *provable* mismatch: the argument's type shares
                // no tag with what the callee accepts. A superset, an `any`
                // result, or an unknown argument (`None`) overlaps the param, so
                // it's never flagged — no false positives.
                //
                // A `NEVER` arg type means "this branch is unreachable" — every
                // intersection with NEVER is NEVER, so it'd warn against
                // *every* param. That's all noise: the code can't execute, so
                // there's no real misuse. Skip. This shows up in pattern-match
                // lowering where a guard has narrowed a variable to a type
                // that has no inhabitants for the current branch.
                if let Some(arg_ty) = expr_ty(heap, arg, ctx) {
                    if arg_ty.is_never() {
                        continue;
                    }
                    if arg_ty.is_disjoint(&param) {
                        let msg = format!(
                            "{}: argument {} expects {}, got {} ({})",
                            name_of(s),
                            i + 1,
                            param,
                            arg_ty,
                            crate::syntax::printer::print(heap, arg),
                        );
                        // Locate to the call form (a Pair the reader positioned).
                        out.push((heap.form_pos(form), msg));
                    }
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
                                out.push((heap.form_pos(form), msg));
                            }
                        }
                    }
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
    check_fn_seeded(heap, items, ctx, out, None);
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
    let sig = sig.filter(|s| s.rest.is_none() && s.params.len() == params.len());
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        match sig.and_then(|s| s.params.get(i)) {
            Some(ty) => scope = scope.bind_sig_param(p, ty.clone()),
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
        if let Some(sig) = ctx.declared_sig(name) {
            if let Some(fn_items) = fn_form_items(heap, value_form) {
                check_fn_seeded(heap, &fn_items, ctx, out, Some(&sig));
                return;
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
fn fn_params(heap: &Heap, form: Value) -> Vec<Symbol> {
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
    let test = items.get(1).copied().unwrap_or(Value::Nil);
    let then_form = items.get(2).copied().unwrap_or(Value::Nil);
    let else_form = items.get(3).copied().unwrap_or(Value::Nil);

    // All three slots are evaluated value positions — a bare unbound symbol in
    // any (`(if typo …)`) is a reference error. then/else use the narrowed ctx,
    // matching how they're walked.
    check_value_leaf(heap, test, form, ctx, out);
    check_into(heap, test, ctx, out);

    let (then_ctx, else_ctx) = match guard_assertion(heap, test, ctx) {
        Some(g) => {
            let then_ctx = ctx.narrow(g.sym, g.ty.clone());
            // **Dead-clause lint.** If the guard narrowed a *sig-typed parameter*
            // to the empty type, this branch can never run — the parameter's
            // declared type is disjoint from what the guard (a `cond` predicate or
            // a `match` literal pattern, reached here via the scrutinee alias)
            // asserts. Gated on a sig-typed param: a literal scrutinee or a
            // compiler-generated guard never involves one, so no false positives.
            if let Some((p, known)) = then_ctx.newly_dead_sig_param(ctx) {
                out.push((
                    heap.form_pos(form),
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
            // Pattern-target binding (post-Step 4 work) — skip narrowing for it
            // but still check the RHS as an expression.
            check_value_leaf(heap, binds[i + 1], form, &scope, out);
            check_into(heap, binds[i + 1], &scope, out);
            i += 2;
            continue;
        };
        let rhs = binds[i + 1];
        // The RHS is an evaluated value position — a bare unbound symbol there
        // (`(let (x typo) …)`) is a reference error.
        check_value_leaf(heap, rhs, form, &scope, out);
        check_into(heap, rhs, &scope, out);
        let rhs_ty = expr_ty(heap, rhs, &scope);
        let rhs_guard = guard_assertion(heap, rhs, &scope);
        scope = scope.bind(name, rhs_ty);
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
