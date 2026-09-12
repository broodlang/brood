//! The unbound-symbol diagnostic and its neighbours: what counts as bound (Step 4), the
//! unrequired-module hint, the stability note a deprecated name carries, the file's own
//! `def` names, and the `check-allow` mask.

use super::*;

/// The prefix every unbound-symbol diagnostic starts with. `SpecialHead::ErrorTesting`
/// matches on it to keep unbound warnings while dropping every other lint inside a
/// `try` / `error-of` / `assert-error` body (KI-67).
pub(in crate::types::check) const UNBOUND_PREFIX: &str = "unbound symbol: ";

/// The `unbound symbol: …` diagnostic text for `nm`, with the foreign-construct
/// hint appended when `nm` names a construct from another Lisp that Brood lacks
/// (so the Brood way is visible at write-time). Shared by the call-head and the
/// value-leaf unbound checks so the two messages can't drift apart.
/// The advisory diagnostic for referencing a name a `(meta …)` marks deprecated or beta
/// (ADR-283), or `None` for an ordinary name.
///
/// **Advisory, not gating.** A deprecation that fails the build is a removal with extra
/// steps: the whole point is to say "this is going away" while the code still works, so
/// `project-advisory-warning?` classifies it as printed-but-not-counted. That is also why
/// this reads the *loaded image* rather than the file — a deprecation is almost always
/// cross-module, and the module that declares it has been loaded by the time its callers
/// are checked.
pub(super) fn stability_msg(heap: &Heap, sym: Symbol) -> Option<String> {
    let meta = heap.name_meta(sym)?;
    let name = name_of(sym);
    if let Some(version) = &meta.deprecated {
        let replacement = match meta.use_instead {
            Some(u) => format!(" — use `{}` instead", name_of(u)),
            None => String::new(),
        };
        return Some(format!(
            "`{name}` is deprecated since {version}{replacement}"
        ));
    }
    meta.beta
        .as_ref()
        .map(|why| format!("`{name}` is beta — {why}"))
}

/// Clip `s` to `max` characters with a trailing ellipsis. A diagnostic names the *reason*;
/// it is not a place to print a type in full. An inferred record shape runs to hundreds of
/// characters — hive's `docs/runnable` rendered a 700-character tuple-of-tuples into one
/// warning, which is unreadable on any terminal and buries the sentence that matters. The
/// full type is a hover away.
pub(super) fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

pub(super) fn unbound_msg(nm: &str) -> String {
    let mut msg = format!("{}{}", UNBOUND_PREFIX, nm);
    // A deliberate rename says where the name went (ADR-304) — the same suffix the
    // runtime error carries, from the same ledger.
    if let Some(hint) = crate::renames::rename_hint(nm) {
        msg.push_str(&hint);
    }
    if let Some(hint) = crate::eval::foreign_construct_hint(nm) {
        msg.push_str(" — ");
        msg.push_str(hint);
    }
    msg
}

/// The debug-only primitives (registered under `#[cfg(debug_assertions)]`, see
/// `builtins/diagnostics.rs` / `builtins/evaluation.rs`): they exist in a dev build but not a
/// release one, so a `nest check` running in a release binary would flag every
/// (legitimate, `bound?`-guarded) test reference as unbound. They're real
/// primitives, so the checker knows their names regardless of the build config —
/// the honest fix for the "release-only phantom unbound" build artifact.
pub(super) fn is_debug_only_primitive(nm: &str) -> bool {
    matches!(nm, "%blob-ptr" | "%blob-strong-count" | "%force-panic")
}

/// The suppression bitmask a `(check-allow :category …)` marker's category
/// keyword names. Unknown / missing → `0` (suppress nothing — a typo'd category
/// is thus a no-op that still lints, never a silent blanket suppression). Keep
/// the recognised names in sync with the `check-allow` docstring.
pub(super) fn lint_allow_mask(category: Option<Value>) -> u8 {
    let Some(Value::Keyword(k)) = category else {
        return 0;
    };
    if value::symbol_is(k, "non-tail-recursion") {
        crate::types::check::ctx::SUPPRESS_NON_TAIL
    } else if value::symbol_is(k, "unreachable-clause") {
        crate::types::check::ctx::SUPPRESS_UNREACHABLE
    } else if value::symbol_is(k, "type-mismatch") {
        crate::types::check::ctx::SUPPRESS_TYPE_MISMATCH
    } else if value::symbol_is(k, "unbound") {
        crate::types::check::ctx::SUPPRESS_UNBOUND
    } else if value::symbol_is(k, "unrequired") {
        crate::types::check::ctx::SUPPRESS_UNREQUIRED
    } else if value::symbol_is(k, "deprecated") {
        crate::types::check::ctx::SUPPRESS_DEPRECATED
    } else {
        0
    }
}

/// A symbol in *reference* position that resolves to nothing — not a local
/// binder, not a syntactic keyword, not a curated stdlib name, and not in the
/// heap's globals (which includes macros and, once the project is loaded,
/// file-local defs). The single predicate behind **both** the call-head and the
/// operand unbound diagnostics, so the two never drift apart.
pub(in crate::types::check) fn is_unbound(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_local(s) || is_globally_bound(heap, s) || curated_sig(s).is_some() {
        return false;
    }
    // A name the enclosing top-level form tests with `(bound? 'name)` is *meant* to be
    // absent from some images — the guard is the handling. Warning there would flag
    // correct code (`std/prelude/tools.blsp`'s `%impl-app?` reading the project ambients).
    if ctx.is_bound_guarded(s) {
        return false;
    }
    // An ambiguous `(:use …)` import (ADR-235) is not "unbound" — it has candidate
    // bindings, just no single one. The compile pass already reports the precise
    // ambiguity ("imported from more than one module"), so don't double-flag it as unbound.
    if heap.ambiguous_import_of(s).is_some() {
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
        crate::types::check::deps::obs_known_ns(heap, &nm[..=slash]);
        if !ctx.module_is_known(&nm[..=slash]) {
            return false;
        }
    }
    true
}

/// **KI-17** — a *user-written* qualified reference `mod/name` that resolves in the
/// loaded image but whose module `mod` the file never `require`s/`:use`s. It works only
/// by load-order luck (another file pulled `mod` in first); reorder or drop that file and
/// it raises `unbound symbol: mod/name` at runtime. Returns `Some(math/mod)` to warn.
///
/// Silent unless the file's reachability set is known ([`Ctx::required_mods`], whole-
/// project mode), the symbol is genuinely *bound* (an unbound one is [`is_unbound`]'s
/// job — the two are mutually exclusive), and the exact reference is *user-written*
/// ([`Ctx::raw_qualified_has`] — never a macro-injected reference to a module the file
/// doesn't mention). Each guard removes a false-positive class, keeping the lint sound.
pub(super) fn unrequired_module(_heap: &Heap, ctx: &Ctx, s: Symbol) -> Option<String> {
    // OBSOLETE since the ADR-227 follow-up: a qualified reference `mod/name` now *infers*
    // `(require 'mod)` — the reference itself loads the module (a qualified macro/call
    // head during macroexpand, any other qualified reference before eval). So a
    // "reference to an unrequired module" can no longer occur: there is no unrequired
    // module to reference. The lint is a permanent no-op. Its reachability scaffolding
    // (`required_mods` / `raw_qualified`) is retained for now — still touched here so it
    // stays wired if ever repurposed — and can be pruned in a later cleanup.
    let _ = (ctx.required_mods(), ctx.raw_qualified_has(&name_of(s)));
    None
}

/// The KI-17 reachability diagnostic text for a reference to unrequired `module`.
pub(super) fn unrequired_msg(module: &str) -> String {
    format!(
        "qualified reference to unrequired module: {module} (add (require '{module}) to this file)"
    )
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
pub(in crate::types::check) fn resolves_to_macro(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_lexical_local(s) {
        return false;
    }
    ctx.is_file_macro(s)
        || matches!(
            crate::types::check::deps::obs_global(heap, s),
            Some(Value::Macro(_))
        )
}

/// A call head nothing can be proven about: a **qualified** `mod/name` whose
/// module is not loaded, so the checker cannot tell a function from a macro.
///
/// [`is_unbound`] already declines to flag such a head — the module may be
/// defined dynamically, or added to `*load-path*` by the program itself before
/// the reference runs. This is the other half of that carve-out. If the head
/// might be a macro, its arguments might be *opaque syntax*, exactly as
/// [`resolves_to_macro`] describes, so the walk must not descend into them
/// either. Without this the checker stays silent about the head it cannot
/// resolve and then reports every mnemonic inside `(mod/asm (movz x0 …) …)` as
/// an unbound symbol — silent about the thing it doesn't know, loud about the
/// things that follow from it.
///
/// Narrow on purpose: a BARE unresolvable head is still walked into, because a
/// bare name that resolves to nothing is a typo the checker should report,
/// arguments and all. Only the unknown-module case is opaque.
pub(in crate::types::check) fn head_is_unresolvable(heap: &Heap, ctx: &Ctx, s: Symbol) -> bool {
    if ctx.is_local(s) || is_globally_bound(heap, s) || curated_sig(s).is_some() {
        return false;
    }
    let nm = name_of(s);
    match nm.rfind('/') {
        Some(slash) => {
            // Record the known-ns query for the Phase-2 cache, as `is_unbound` does:
            // this file's verdict depends on whether the prefix is known.
            crate::types::check::deps::obs_known_ns(heap, &nm[..=slash]);
            !ctx.module_is_known(&nm[..=slash])
        }
        None => false,
    }
}

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
pub(in crate::types::check) fn collect_def_names(heap: &Heap, form: Value, ctx: &mut Ctx) {
    // Deep-form stack safety — same stacker remedy as check_into above.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_def_names_inner(heap, form, ctx)
    })
}

pub(super) fn collect_def_names_inner(heap: &Heap, form: Value, ctx: &mut Ctx) {
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
            // Record the arity the *definition* admits, so a call to a same-file
            // function is arity-checked at all (the file isn't loaded, so
            // `sigs::arity_of` sees nothing) and so a `(sig …)` that disagrees with
            // the definition can't silently supply a wrong one.
            if let Some(arity) = items.get(2).and_then(|&v| fn_form_arity(heap, v)) {
                ctx.add_file_arity(name, arity);
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

/// A bare symbol in a NON-FINAL body position: evaluated, discarded, and — since reading a
/// name has no effect — dead. It is worth a diagnostic on its own, and it is also how
/// `f(x)` call syntax reads to a Lisp reader: `string?(num)` is not a call, it is the symbol
/// `string?` followed by the list `(num)`, so the symbol lands in exactly this position.
/// That is the slip this catches, and `foreign_construct_hint` cannot: it keys on unbound
/// NAMES, and `string?` is bound.
///
/// `_`-prefixed names are exempt (the deliberate-discard convention), and so is a symbol
/// that is not resolvable — an unbound one already has its own, better diagnostic.
pub(super) fn lint_discarded_symbols(
    heap: &Heap,
    body: &[Value],
    parent: Value,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if body.len() < 2 || ctx.is_suppressed(crate::types::check::ctx::SUPPRESS_UNBOUND) {
        return;
    }
    for (i, &form) in body.iter().enumerate().take(body.len() - 1) {
        let Value::Sym(sym) = form else { continue };
        let name = name_of(sym);
        if name.starts_with('_') || is_gensym_sym(sym) || is_unbound(heap, ctx, sym) {
            continue;
        }
        let followed_by_list = matches!(body.get(i + 1), Some(Value::Pair(_)));
        // A QUALIFIED name read for effect is a real idiom, not dead code: `mod/name`
        // auto-loads its module (ADR-227), which is exactly why
        // `(do rand/int term/raw-enter … nil)` sits at the top of `introspection_test` —
        // "referencing the function values auto-requires the module without calling them",
        // as its own comment says. So reading a name is NOT always effect-free, and the
        // premise this lint started from was wrong. It stays exempt unless a list follows
        // it, which is the `mod/f(x)` shape rather than the auto-load one.
        if name.contains('/') && !followed_by_list {
            continue;
        }
        let msg = if followed_by_list {
            format!(
                "{name} is evaluated here and discarded — and the form after it is a list, \
                 which is how `{name}(x)` reads to the reader: two forms, not a call. \
                 A call is `({name} x)`"
            )
        } else {
            format!("{name} is evaluated here and discarded — reading a name has no effect")
        };
        out.push((arg_pos(heap, form, parent), msg));
    }
}
