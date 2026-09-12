//! The recursive walk: visit every sub-form, open the right scope at each binder, and
//! at every call site cross-check arity + per-argument type against what the callee
//! accepts. This file is `check_into` — the dispatch over special heads and the generic
//! call-form path — with the rest as child modules: `shape` (the syntax-shape readers
//! the whole checker shares), `calls` (call-site argument and callback checking),
//! `binders` (the `fn`/`def`/`defn`/`if`/`let` checkers), `unbound` (the unbound-symbol
//! and unrequired-module diagnostics) and `impls` (ability impl return checking).

use super::ctx::{Ctx, PathKey};
use super::guards::{
    and_conjunct_guards, find_redundant_clause, guard_assertion, is_syntactic_keyword,
    literal_eq_test_raw, match_exhaustiveness_gap, or_same_var_narrowing, path_guard_assertion,
    render_literal_pattern,
};
use super::infer::{expr_ty, global_value_ty};
use super::sigs::{
    arity_of, arity_str, curated_sig, declared_heap_overload, declared_heap_sig,
    declared_heap_value_ty, infer_overload_of, is_globally_bound, sig_of,
};
use crate::core::heap::{Heap, SymbolMap};
use crate::core::keywords as kw;
use crate::core::value::{self, Arity, Symbol, Value};
use crate::error::Pos;
use crate::types::{GradualTy, Sig, Ty};
use std::collections::HashSet;
use std::sync::LazyLock;

mod binders;
mod calls;
mod impls;
mod shape;
mod unbound;

pub(in crate::types::check) use binders::*;
pub(in crate::types::check) use calls::*;
pub(in crate::types::check) use impls::*;
pub(in crate::types::check) use shape::*;
pub(in crate::types::check) use unbound::*;

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
        if !ctx.is_suppressed(super::ctx::SUPPRESS_DEPRECATED) {
            if let Some(msg) = stability_msg(heap, s) {
                out.push((heap.form_pos_only(parent), msg));
            }
        }
    }
}

/// Walk a quasiquote template looking for **escapes** — `~x` / `~@x` at the current
/// nesting level — and check each escaped form as ordinary code. Everything else in a
/// template is data: a bare `foo` inside `` `(foo ~x) `` is the *symbol* `foo`, not a
/// reference to it, so it must never be flagged.
///
/// `level` is the quasiquote depth: a nested `` ` `` raises it, an `~` lowers it, and
/// only an escape that brings the depth back to 0 is code *here* (an inner template's
/// `~` belongs to that template). Containers are walked too — `` `[~a {:k ~b}] `` — for
/// the same reason KI-70 gave.
fn check_unquoted(
    heap: &Heap,
    form: Value,
    level: u32,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        match form {
            Value::Vector(id) => {
                for item in heap.vector(id).to_vec() {
                    check_unquoted(heap, item, level, ctx, out);
                }
            }
            Value::Map(mid) => {
                for (k, v) in heap.map_entries(mid) {
                    check_unquoted(heap, k, level, ctx, out);
                    check_unquoted(heap, v, level, ctx, out);
                }
            }
            Value::Pair(_) => {
                let Some(items) = list_items(heap, form) else {
                    return;
                };
                if let Some(&Value::Sym(h)) = items.first() {
                    if value::symbol_is(h, kw::QUASIQUOTE) {
                        for &it in &items[1..] {
                            check_unquoted(heap, it, level + 1, ctx, out);
                        }
                        return;
                    }
                    if value::symbol_is(h, kw::UNQUOTE) || value::symbol_is(h, kw::UNQUOTE_SPLICING)
                    {
                        for &it in &items[1..] {
                            if level <= 1 {
                                check_into(heap, it, ctx, out); // code, at this level
                            } else {
                                check_unquoted(heap, it, level - 1, ctx, out);
                            }
                        }
                        return;
                    }
                }
                for &it in &items {
                    check_unquoted(heap, it, level, ctx, out);
                }
            }
            _ => {}
        }
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
    // A vector or map **literal in value position is evaluated code**: `[:tag (f x)]`
    // calls `f`, and so does `{:k (f x)}`. This walk used to return here for anything
    // that is not a `Pair`, so every form nested inside `[…]` / `{…}` was invisible to
    // every lint — and Hiccup-shaped code (hive's entire web layer, `std/editor/*`) is
    // written that way. `(str (max 2 …))` sat in hive's `/docs` renderer long after
    // `max` moved to `math`, with `nest check` green the whole time; only rendering the
    // page raised it. Same class as KI-67, one level out: not a form that suppressed the
    // lint, a form the walk never reached.
    //
    // Descending is safe on both counts that would otherwise cost false positives:
    // the checker runs on **macroexpanded** forms, so a `match` pattern vector has
    // already been lowered to `let`/`if` binders and no binder vector survives in value
    // position; and `quote`/`quasiquote`/`comment` return at `SpecialHead::SkipBody`
    // above without ever handing their data down here.
    match form {
        Value::Vector(vid) => {
            for item in heap.vector(vid).to_vec() {
                check_into(heap, item, ctx, out);
            }
            return;
        }
        Value::Map(mid) => {
            for (k, v) in heap.map_entries(mid) {
                check_into(heap, k, ctx, out);
                check_into(heap, v, ctx, out);
            }
            return;
        }
        _ => {}
    }
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    capture_arg_ty(heap, form, &items, ctx);
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
                        && !g
                            .clone()
                            .consistent_with_mode(relax_param_for_arg(pty), ctx.strict())
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
                SpecialHead::Quasiquote => {
                    for it in items.iter().skip(1) {
                        check_unquoted(heap, *it, 1, ctx, out);
                    }
                    return;
                }
                SpecialHead::ErrorTesting => {
                    // Walk the body into a scratch buffer and keep ONLY the
                    // unbound-symbol diagnostics. Filtering at the collection
                    // point rather than gating lint-by-lint is deliberate: a
                    // lint added later is suppressed here by default, which is
                    // the right default for a form whose whole purpose is to
                    // exercise a failure. The prefix is the one `unbound_msg`
                    // builds, the single constructor for both unbound sites.
                    let mut inner_out = Vec::new();
                    for it in items.iter().skip(1) {
                        check_into(heap, *it, ctx, &mut inner_out);
                    }
                    out.extend(
                        inner_out
                            .into_iter()
                            .filter(|(_, m)| m.starts_with(UNBOUND_PREFIX)),
                    );
                    return;
                }
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
        // **A variable whose own type is an arrow describes the call it heads.** A
        // parameter declared `(sig apply-it ((int -> string) -> any))` carries a full
        // signature, and `(f "x")` inside the body is the only site that can use it —
        // without this the arrow was inert in both directions: the result had no type
        // (`infer.rs`) and the arguments went unchecked here, so declaring one bought
        // nothing at all. Checked FIRST because a local shadows any global of the same
        // name, and `ctx.get` only answers for a variable actually in scope.
        let local_ty = ctx.get(s);
        let local_arrow = local_ty.as_ref().and_then(Ty::as_arrow).cloned();
        let sig = local_arrow
            .clone()
            .or_else(|| declared.clone())
            .or_else(|| {
                (!ctx.is_lexical_local(s) && !ctx.is_file_global(s))
                    .then(|| sig_of(heap, s))
                    .flatten()
            })
            .or_else(|| {
                // A same-file function's inferred sig (Pass 2.8, ADR-190) — now carrying param
                // demands, so a same-file caller's args are checked (the file isn't loaded, so
                // `sig_of`'s heap path above can't see it). A lexical local shadows the global.
                (!ctx.is_lexical_local(s))
                    .then(|| ctx.inferred_fn_sig(s))
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
        // **An arrow parameter's arity is exact.** `is_lexical_local` skips the whole
        // computation below because a local's arity is normally unknown — but when the
        // local's own TYPE is an arrow, the arrow says precisely how many arguments it
        // takes. `(sig apply-it ((int -> string) -> any))` then catches `(f 1 2)` in the
        // body, which is a certain error rather than a gradual one: the caller of
        // `apply-it` had to supply a one-argument function to satisfy that parameter, so
        // calling it with two always raises. Without this the arrow described the call's
        // types (ADR-273) but not its shape, which is half a contract.
        let arity = if let Some(sg) = &local_arrow {
            Some(arity_of_sig(sg))
        } else if ctx.is_lexical_local(s) {
            None
        } else {
            (!ctx.is_file_global(s))
                .then(|| arity_of(heap, s))
                .flatten()
                // The def site of a same-file function — the file isn't loaded, so
                // `arity_of` above can't see it. Read *before* the declared sig, so a
                // `(sig …)` that disagrees with the definition can't supply a wrong
                // arity: the definition is what the call actually meets at run time.
                .or_else(|| ctx.file_arity(s))
                .or_else(|| {
                    declared
                        .filter(|sg| sg.rest.is_some() || !ctx.is_variadic_global(s))
                        .map(|sg| arity_of_sig(&sg))
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
        // A CALLED name gets the same stability diagnostic a referenced one does — the
        // call is the commoner shape by far, and the value-slot check above never sees it.
        if !ctx.is_suppressed(super::ctx::SUPPRESS_DEPRECATED) {
            if let Some(msg) = stability_msg(heap, s) {
                out.push((heap.form_pos_only(form), msg));
            }
        }
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
                // Same wording as the RUNTIME's arity error (`eval::arity_message`), and
                // the same parameter names: the checker used to say "wrong number of
                // arguments — expected 1, got 0" where running it said "expected 1
                // argument, got 0", so one defect read as two unrelated messages.
                // The SAME function the runtime raises with, not a second implementation
                // of the same sentence. The checker used to say "wrong number of arguments
                // — expected 1, got 0" where running it said "expected 1 argument, got 0";
                // a comment promising the two will not drift is not a mechanism, calling
                // one function is.
                out.push((
                    heap.form_pos_only(form),
                    crate::eval::arity_message(
                        &name_of(s),
                        a.min,
                        a.max,
                        argc,
                        &super::sigs::param_names_of(heap, s).unwrap_or_default(),
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
        // A SPECIAL FORM's name sitting in an ARGUMENT slot: the commonest paren slip in
        // the language. `(reduce xs '() fn (acc token) acc)` reported only
        // `unbound symbol: acc`, twice — a symptom two levels down — and said nothing about
        // the `fn` that lost its parentheses. `is_unbound` deliberately exempts a syntactic
        // keyword (it is not a global and never will be), which is why nothing spoke up.
        //
        // Checked HERE, in the call walk, rather than in `check_value_leaf`: that path is
        // gated on whole-file operand checking and on `evaluates_args`, and the editor's
        // entry (`check-string-structured`, which is what bedit's `:diagnostics` service
        // calls) did not reach it — the diagnostic would have shown on the command line and
        // not where the slip is actually made. A local of that name is exempt: several
        // keyword names are ordinary words, and both `std/editor/keymap.blsp` and
        // `std/prelude/control.blsp` bind a local called `binding`.
        if !ctx.is_suppressed(super::ctx::SUPPRESS_UNBOUND) {
            for &arg in &items[1..] {
                if let Value::Sym(a) = arg {
                    let nm = name_of(a);
                    if !ctx.is_local(a) && super::guards::is_syntactic_keyword(&nm) {
                        out.push((
                            arg_pos(heap, arg, form),
                            format!(
                                "{nm} is a special form, not a value — it has to be called, \
                                 as `({nm} …)`; a missing pair of parentheses turns its own \
                                 parts into arguments here"
                            ),
                        ));
                    }
                }
            }
        }

        if value::symbol_is(s, "throw") && items.len() == 2 {
            if let Some(msg) = match_exhaustiveness_gap(heap, items[1], ctx) {
                out.push((heap.form_pos_only(form), msg));
            }
        }

        // A type predicate whose argument CANNOT hold that type: the test can never be
        // true, so the branch behind it is dead. This is the check ADR-315 wanted for the
        // failure channel — `(failure? n)` on something that has no way to be a failure is
        // a guard the author believes is doing work and is not — and it costs nothing to
        // ask it of every predicate in the table, since `Ty::tested_by` already names what
        // each one proves.
        //
        // Sound because `GradualTy::bound` is an UPPER bound ("every materialisation is
        // ⊆ bound", asserted by `soundness_oracle`): if the bound is disjoint from the
        // tested type, no runtime value can satisfy the predicate. A `dynamic` bound is
        // therefore fine to judge — dynamic means it may materialise NARROWER, never
        // wider. `ANY` and `NEVER` are skipped: the first knows nothing, the second marks
        // a branch a guard already proved unreachable.
        // A GENSYM argument is not the author's test. `ok->`, `with` and the `match`
        // lowering all emit `(if (pred g__N) …)` over a temporary they made up: where the
        // checker can prove that step's value is not of the tested type, the guard is
        // indeed dead — but the macro had to emit it and there is nothing to fix. Ten of
        // the first run's `failure?` hits were `ok->`/`with`'s own expansion and twelve
        // `pair?` hits were `match`'s. Same exemption the unused-binder lints already make.
        //
        // And only a NAME or a computed expression is worth judging. A predicate applied
        // to a written-out literal — `(decimal? 1.5)`, `(ref? 0)` — is either a deliberate
        // negative assertion (which is what 53 of the first run's 57 remaining hits were,
        // in the predicate test files) or a mistake already visible on the line. The lint
        // earns its keep where the type came from INFERENCE and the reader cannot see it.
        // …though only for the WIDER predicate set. `failure?` had no literal hits at all
        // across std/ + tests/, and `(failure? 42)` is precisely the "checked where it
        // cannot exist" case worth naming, so the literal skip does not apply to it.
        let judged = value::symbol_is(s, "failure?")
            || matches!(items.get(1), Some(Value::Sym(_)) | Some(Value::Pair(_)));
        let author_wrote_it =
            judged && !matches!(items.get(1), Some(&Value::Sym(a)) if is_gensym_sym(a));
        if items.len() == 2
            && author_wrote_it
            && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
        {
            if let Some(tested) = super::guards::predicate_guard_ty(heap, Some(ctx), s) {
                let bound = gradual_of(heap, items[1], ctx).bound;
                if !bound.is_never() && bound != Ty::ANY && bound.is_disjoint(&tested) {
                    out.push((
                        arg_pos(heap, items[1], form),
                        format!(
                            "{}: this can never be true — {} is {}, which is never {}",
                            name_of(s),
                            elide(&crate::syntax::printer::print(heap, items[1]), 60),
                            elide(&bound.to_string(), 80),
                            tested,
                        ),
                    ));
                }
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
                // A function LITERAL in a slot with no room for a function: a
                // lambda whose result can't be inferred (`(fn (x) x)`) has no
                // arrow type, so it reads as dynamic and the gradual check below
                // stays quiet — but its TAG is never in doubt. If the parameter
                // is disjoint from fn/native entirely, the call is wrong whatever
                // the lambda returns. This is the check that catches a
                // pre-data-first `(map (fn (x) x) xs)` argument order, which the
                // 0.20 migration showed failing only at runtime.
                let fn_literal = matches!(arg, Value::Pair(_))
                    && list_items(heap, arg)
                        .as_deref()
                        .and_then(|it| it.first().copied())
                        .is_some_and(|h| matches!(h, Value::Sym(hs) if is_fn_head(hs)));
                if fn_literal
                    && param.is_disjoint(
                        &Ty::of(crate::core::value::Tag::Fn)
                            .union(Ty::of(crate::core::value::Tag::Native)),
                    )
                    && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                {
                    out.push((
                        arg_pos(heap, arg, form),
                        format!(
                            "{}: argument {} expects {}, got a function ({})",
                            name_of(s),
                            i + 1,
                            param,
                            crate::syntax::printer::print(heap, arg),
                        ),
                    ));
                    continue;
                }
                let g = gradual_of(heap, arg, ctx);
                // Relax the parameter for the membership test in the two places the
                // lattice deliberately under-approximates (see `relax_param_for_arg`),
                // so the advisory check never misfires; the original `param` is still
                // what the message reports.
                let param_relaxed = relax_param_for_arg(&param);
                if !g.bound.is_never()
                    && !g.clone().consistent_with_mode(param_relaxed, ctx.strict())
                    && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                {
                    let msg = format!(
                        "{}: argument {} expects {}, got {} ({})",
                        name_of(s),
                        i + 1,
                        crate::types::check::annot::display_ty(&param),
                        crate::types::check::annot::display_ty(&g.bound),
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
                        // Callback **parameter-type** check. The arity check above asks
                        // whether the callback can be called at all; this asks whether it
                        // can accept what it will be handed. A `sig`-declared higher-order
                        // parameter (`(sig g (((int -> int)) -> int))`) was annotated and
                        // then not enforced — passing `string/length` to it was silent.
                        //
                        // Disjointness only, per position, and never on the *result*: an
                        // inferred return over-approximates, so comparing results would
                        // false-positive at every call site (a `(any) -> any` callback is
                        // not a subtype of `(int) -> int`, but it is perfectly valid).
                        // Sound with an inferred callback sig too: an inferred parameter
                        // demand is a *superset* of what the function really accepts, so
                        // disjoint-from-the-superset is disjoint from the truth.
                        if let Some(cb) = callback_sig(heap, arg, ctx)
                            .or_else(|| lambda_sig_under(heap, arg, expected, ctx))
                        {
                            for (k, wanted) in expected.params.iter().enumerate() {
                                let Some(accepts) = cb.param(k) else { continue };
                                if wanted.is_never()
                                    || accepts.is_never()
                                    || !wanted.is_disjoint(&accepts)
                                    || ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                                {
                                    continue;
                                }
                                let msg = format!(
                                    "{}: argument {} is a callback handed {} at position {}, \
                                     but {} takes {} there",
                                    name_of(s),
                                    i + 1,
                                    wanted,
                                    k + 1,
                                    callback_desc(arg),
                                    accepts,
                                );
                                out.push((arg_pos(heap, arg, form), msg));
                            }
                            // …and the **result**. This was left out on the grounds that
                            // an inferred return over-approximates, so comparing it by
                            // *subtyping* would false-positive at every call site — a
                            // `(any) -> any` callback is not a subtype of `(int) -> int`
                            // and is perfectly valid. Disjointness has no such problem
                            // and is sound in the same way the parameter direction is:
                            // the inferred return is a superset of the truth, so if the
                            // superset shares nothing with what the caller will do with
                            // it, neither does the truth.
                            let (wants, gives) = (&expected.ret, &cb.ret);
                            if !wants.is_any()
                                && !gives.is_any()
                                && !wants.is_never()
                                && !gives.is_never()
                                && wants.is_disjoint(gives)
                                && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                            {
                                let msg = format!(
                                    "{}: argument {} is a callback whose result is used as \
                                     {}, but {} returns {}",
                                    name_of(s),
                                    i + 1,
                                    wants,
                                    callback_desc(arg),
                                    gives,
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
                // …and, failing a declaration, the arms *inferred* from a multi-arm
                // definition: same-file first (the file isn't loaded), else the loaded
                // closure's. A multi-arm callee used to escape argument checking
                // entirely, since it has no single `Sig` for the per-argument loop.
                .or_else(|| ctx.inferred_overload(s))
                .or_else(|| {
                    (!ctx.is_file_global(s))
                        .then(|| infer_overload_of(heap, s))
                        .flatten()
                })
            {
                let arg_tys: Vec<Option<Ty>> =
                    items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                if overload_arg_mismatch(&arms, &arg_tys)
                    && !ctx.is_suppressed(super::ctx::SUPPRESS_TYPE_MISMATCH)
                {
                    out.push((
                        heap.form_pos_only(form),
                        format!(
                            "{}: no clause accepts these arguments — the clauses take {}",
                            name_of(s),
                            clause_domains_desc(&arms, items.len() - 1),
                        ),
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
    // A `do`'s non-final forms are a body sequence, so a bare symbol among them is
    // evaluated and discarded — see `lint_discarded_symbols`, which also recognises the
    // `f(x)` shape that lands there.
    if matches!(items.first(), Some(&Value::Sym(s)) if value::symbol_is(s, kw::DO)) {
        lint_discarded_symbols(heap, &items[1..], form, ctx, out);
    }
    if !head_is_macro {
        // A `fold`/`reduce` callback written as a `fn` literal is walked with its
        // parameters SEEDED: the accumulator to the fold's own result type (the fixpoint
        // `infer::seq_aware_call_ty` computes — a superset of every accumulator value, by
        // induction) and the element to the collection's element type. Unseeded, `h` in
        // `(fold s 5381 (fn (h c) (bit/xor (* h 31) …)))` read as `any` and `(* h 31)` as
        // `number`, while the fold as a whole was already known to be an int.
        let seeded_callback = fold_callback_seed(heap, form, &items, ctx)
            .or_else(|| element_callback_seed(heap, &items, ctx));
        // A body sequence threads its scope: a **guard that diverges** narrows every form
        // after it (see [`diverging_guard_scope`]). Only for a `do` — in any other form
        // the items are arguments, evaluated in one scope, and there is no "after".
        let is_body = matches!(items.first(), Some(&Value::Sym(s)) if value::symbol_is(s, kw::DO));
        let mut seq_ctx = ctx.clone();
        for (i, &item) in items.iter().enumerate() {
            let item_ctx = if is_body { &seq_ctx } else { ctx };
            match &seeded_callback {
                Some((idx, sig)) if *idx == i => {
                    if let Some(fn_items) = list_items(heap, item) {
                        check_fn_bound(heap, &fn_items, item_ctx, out, &sig.params);
                    }
                }
                _ => check_into(heap, item, item_ctx, out),
            }
            if is_body {
                if let Some(next) = diverging_guard_scope(heap, item, &seq_ctx) {
                    seq_ctx = next;
                }
            }
        }
    }
}
