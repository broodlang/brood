//! Step 4: a small **advisory** type checker — the consumer of the `Ty` lattice,
//! so the type system actually *does* something.
//!
//! It walks a macro-expanded form and warns when a call passes an argument that
//! is *provably* the wrong type — its type is **disjoint** from what the callee
//! accepts (`(first 5)`, `(+ 1 "x")`). Disjointness (not subtyping) is the rule,
//! so a superset (`number` where `int` is wanted), an `any` result, or an
//! unknown argument all overlap the expected type and are never flagged — **no
//! false positives**. It never raises and never gates — it returns warnings
//! (contract point #5).
//!
//! ## Module map
//!
//! Split by concern, not by special form:
//! - [`ctx`] — the `Ctx` value the walk threads, recording binders, type
//!   narrowings, guard aliases, and file-local globals.
//! - [`sigs`] — where signatures + arities come from (primitive / curated /
//!   one-step-inferred).
//! - [`guards`] — predicates on forms: which heads are syntax keywords,
//!   which `if`-tests are recognisable guards, what an expression's type is.
//! - [`walk`] — the recursive `check_into` and the per-special-form helpers
//!   (`if`/`let`/`fn`/`def`/`defn`) plus `collect_def_names`.
//! - [`annot`] — reads `(sig …)` declarations off the *un-expanded* tree, so a
//!   user-declared signature seeds the checks for that name.
//! - [`protocol`] — protocol / behaviour conformance: `defprotocol` /
//!   `defbehaviour` / `defimpl` / `(:implements …)` checked for missing or
//!   wrong-arity ops.
//! - [`recursion`] — the non-tail self-recursion lint (deep non-tail recursion
//!   overflows the green-process stack).
//!
//! ## Where signatures come from (Step 3)
//!
//! Three sources, simplest-first — a bounded, sound inferencer rather than a full
//! unification engine (`docs/types.md`):
//!
//! 1. **Primitives** — every [`NativeFn`](crate::core::value::NativeFn) carries
//!    a `Sig` ([contract point #6, enforced](../docs/types.md#compatibility-contract))
//!    so the checker just reads it from the global env (see
//!    [`sigs::primitive_sig`]). There is no parallel table to maintain.
//! 2. **Curated stdlib** — a small hand-vetted table for the variadic /
//!    `reduce`-based / higher-order Brood closures the checker can't infer but
//!    that matter (`+ - * / < <= > >= mod map filter reduce`; see
//!    [`sigs::curated_sig`]). Each is a Brood `defn`, but its sig is pinned by hand.
//! 3. **Inference** ([`sigs::infer_sig`]) — a bounded, sound, one-step-deep inferencer
//!    (no unification / global solve). **Parameters** come from *unconditional* type
//!    demands across the body (a guarded use never constrains a param). **The return** is
//!    the body tail's type via `expr_ty`, which unions `if`/`cond`/`let`/`do`/`case`
//!    results — so a branchy body is inferred, not skipped. Plus: a self-recursive call in
//!    a branch result contributes ⊥ (recursion infers from base cases), and a
//!    multi-arity / `&optional` / rest closure gets a params-less return-only sig (the
//!    union of its arm tails; arity is checked separately). Sound throughout — params
//!    under-constrained, returns over-approximated, callees looked up non-inferring — so
//!    zero false positives (see [`sigs::sig_of`]).
//!
//! Argument types in a call come from literals, nested calls with a known
//! return type, and **a context-tracked map of local-variable narrowings**:
//!
//! - A `let`/`let*` binding's RHS contributes its `expr_ty` as the variable's
//!   type (so `(let (x 1) (first x))` flags `first` — `x` is known `int`).
//! - An `if`'s test is matched against the predicate-narrowing table
//!   ([`Ty::tested_by`]). On a `(pred? sym)` test the *then*-branch narrows
//!   `sym` to `tested_by(pred)`, the *else*-branch to its complement; a leading
//!   `(not …)` flips the assertion. Bindings inside a branch override the
//!   narrowing as ordinary shadowing.
//!
//! The *disjointness* check's vocabulary is `Option<Ty>` (known / unknown), not
//! `GradualTy` — it only needs "do I know this type?". The one place `GradualTy`
//! *is* used is the **gradual-assignment check** (`walk::gradual_of` + `check_def`):
//! `(def x …)` against a non-arrow `(sig x T)` uses **consistent subtyping**, where a
//! bounded dynamic (`dynamic_within(t)` for a declared-typed redefinable global) is
//! the thing `Option<Ty>` can't express. Forms inside `try` /
//! `error-of` / `assert-error` are skipped (they deliberately exercise failures).
//!
//! ## Beyond type misuse
//!
//! The walk also emits two non-type diagnostics, sharing the same scope
//! infrastructure:
//!
//! - **Arity**: a call whose argument count isn't admitted by the callee's
//!   declared `Arity` (from [`NativeFn`](crate::core::value::NativeFn) for a
//!   primitive, or from `Closure.{params, optionals, rest}` for a Brood
//!   closure). See [`sigs::arity_of`].
//! - **Unbound symbols**: a call head that resolves to nothing — not a
//!   primitive, not a curated stdlib closure, not in local scope (fn/let), not
//!   a file-local def, not a syntactic keyword, and not in the heap's globals.
//!   Driven by [`Ctx::is_local`](ctx::Ctx::is_local) (the local + file-global
//!   view) plus a global-env lookup. Scope is honoured: `fn`/`lambda`/`defn`/
//!   `defmacro` bind their params into `Ctx` before walking the body, and
//!   [`check_file`] accumulates top-level `def`/`defn`/`defmacro` / `defdyn`
//!   names across the forms in a file.
//!
//! Inference now covers control-flow returns (`if`/`cond`/`let`/`do`/`case`), recursion,
//! multi-arity/variadic closure *returns*, a file's own functions (`check_file`'s Pass 2.8
//! fixpoint, form-based since the file isn't loaded while checked), inferred *parameters*
//! (ADR-190 — a caller's arguments are checked against a derived param type), **`and`/`or`-
//! chained guard narrowing** (every `and` conjunct narrows the then-branch; a same-variable
//! `or` narrows both branches), and higher-order-callback results (a `(map f xs)` element
//! type flows from `f`'s return). The one remaining deferred piece is **per-arm parameter
//! checking of a *multi-arity* callee** — a multi-arity closure still gets a params-less
//! return-only sig, so a call's arguments aren't checked against the matching arm. Leaving it
//! is sound (a missed check is a false negative, never a false positive); closing it would
//! need an inferred-overload path plus per-argc arm selection in the call-check, for marginal
//! value (ADR-011). The checker runs automatically as the pre-flight in `brood <file>` /
//! `nest test` / `nest run` / `nest check`; the in-process entry points are [`check_file`]
//! (whole file) and the `(check 'form)` builtin (a fragment).
//!
//! **Operand-position unbound symbols.** The unbound-symbol diagnostic fires on
//! both a combination's *head* and its *operand / value* positions — `(+ 1 typo)`,
//! `(def x typo)`, `(if typo …)`, `(let (a typo) …)`. An operand leaf is only
//! flagged when the enclosing head is a *known non-macro callee* (a primitive,
//! curated/known closure, or lexical local — see [`walk`]'s `evaluates_args`), so
//! an unexpanded macro argument is never mistaken for a value reference. It is
//! further gated to **whole-file mode** ([`check_file`] sets
//! [`Ctx::enable_operand_checks`](ctx::Ctx::enable_operand_checks)): there every
//! top-level def is accumulated and the project image is loaded, so an unresolved
//! operand is genuinely unbound — whereas a bare fragment (`(check 'form)` / a
//! REPL snippet) keeps free operand variables ambiguous, flagging only the head.
//! All of it reuses the one `is_unbound` predicate, so head and operand checks
//! can't drift.

pub mod annot;
mod ctx;
pub(super) mod deps;
mod exhaustive;
mod guard_effects;
mod guards;
mod infer;
mod protocol;
mod recursion;
mod sigs;
mod walk;

use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self as value, Arity, Symbol, Value};
use crate::error::Pos;
use crate::types::{Sig, Ty};

use ctx::Ctx;
use walk::{check_into, collect_all_syms, collect_def_names, list_items};

/// True when `form` is a top-level `(require …)` call — the one form the
/// checker pre-evaluates so a module's macros (e.g. `defserver` from
/// `std/proc/gen.blsp`) are resolvable for the rest of the file.
fn is_require_form(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, "require-one");
        }
    }
    false
}

/// Is module `name` already loaded — i.e. present in the `*features*` registry the
/// runtime's `require-one` consults? The checker's "do I need to load this `(:use …)`
/// target?" test (see `setup_check_imports`).
///
/// Deliberately the *feature* record and not "does some `name/…` global exist": a std
/// module can share its namespace with kernel primitives (`file/slurp` & co. exist with
/// no `std/file.blsp` loaded), and the presence test then reports loaded for a module
/// whose Brood-level definitions are entirely absent.
///
/// A read, never a load. `false` whenever the answer can't be read off the image
/// (no `*features*`, a non-map value) — the safe direction, since the caller's
/// response is an idempotent `require-one`.
fn feature_loaded(heap: &mut Heap, name: &str) -> bool {
    // Allocate the key BEFORE reading the map handle: an allocation must not run
    // between reading `mid` and using it (this runs under the checker's GC block, so
    // nothing collects here, but the ordering keeps that independent of the block).
    let key = heap.alloc_string(name);
    let features = heap.env_get(heap.global(), value::intern("*features*"));
    match features {
        Some(Value::Map(mid)) => heap.map_get(mid, key).is_some(),
        _ => false,
    }
}

/// Qualify a `(sig …)` declaration's target name to the file's namespace, the way
/// the resolve pass qualifies a def head — but only with the same *positive
/// evidence* the resolver requires: the file actually defines `ns/name` (recorded
/// as a file-global in pass 2). A name the file doesn't define (an imported/prelude
/// name), an already-`ns/`-qualified name, or an ambient `*earmuffed*` name is left
/// as written, so it still matches the bare/qualified head the call resolves to.
fn qualify_decl_name(ctx: &Ctx, file_ns: Option<&str>, name: Symbol) -> Symbol {
    match file_ns {
        Some(ns) => {
            let qualified = crate::eval::macros::qualify_name(ns, name);
            if ctx.is_file_global(qualified) {
                qualified
            } else {
                name
            }
        }
        None => name,
    }
}

/// Parse `form` as any of the four `(sig …)` declaration shapes (arrow, arrow with
/// type-variables, overload, or non-arrow value type) and record it in `ctx` under
/// the namespace-qualified name ([`qualify_decl_name`]). Shared by pass 2.5's two
/// sources — the un-expanded top-level `(sig …)` forms and the `(sig …)` forms
/// reconstructed from `%register-sig` in the expanded tree.
fn register_declared_sig(heap: &Heap, ctx: &mut Ctx, file_ns: Option<&str>, form: Value) {
    if let Some((name, sig)) = annot::parse_sig_decl(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_sig(qn, sig);
    }
    if let Some((name, sv)) = annot::parse_sig_decl_with_vars(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_sig_with_vars(qn, sv);
    }
    // An overloaded sig — `(and (int -> int) (bool -> bool))` — has no single `Sig`,
    // so `parse_sig_decl` above yields nothing for it; record it separately (ADR-116).
    if let Some((name, sigs)) = annot::parse_sig_decl_overload(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_overload(qn, sigs);
    }
    // Non-arrow `(sig x T)` value-type declarations — consumed by the gradual-
    // assignment check on `(def x …)` (the first `GradualTy` consumer).
    if let Some((name, ty)) = annot::parse_value_sig_decl(heap, form) {
        let qn = qualify_decl_name(ctx, file_ns, name);
        ctx.add_declared_value_ty(qn, ty);
    }
}

/// A surface `(defn name clause…)` / `(defn- name clause…)` whose tail is entirely
/// arity clauses — returning the name and those clauses. `None` for a single-clause
/// definition (which the ordinary parameter inference already reads) or any other form.
fn defn_clauses(heap: &Heap, form: Value) -> Option<(Symbol, Vec<Value>)> {
    let items = list_items(heap, form)?;
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    if !value::symbol_is(head, crate::core::keywords::DEFN)
        && !value::symbol_is(head, crate::core::keywords::DEFN_PRIVATE)
    {
        return None;
    }
    let Some(&Value::Sym(name)) = items.get(1) else {
        return None;
    };
    let rest = items.get(2..)?;
    // A leading docstring sits before the clauses, as in a multi-clause `fn`.
    let rest = match rest.first() {
        Some(Value::Str(_)) if rest.len() > 1 => &rest[1..],
        _ => rest,
    };
    if rest.len() < 2
        || !rest
            .iter()
            .all(|&f| crate::eval::macros::is_arity_clause(heap, f))
    {
        return None;
    }
    Some((name, rest.to_vec()))
}

/// Do the two arities overlap — is there an argument count both admit? The
/// question a sig-vs-definition mismatch turns on: only a *disjoint* pair is
/// provably wrong (a multi-arm `defn` annotated with one arm's arrow overlaps,
/// and must stay silent).
fn arities_overlap(a: Arity, b: Arity) -> bool {
    let a_max = a.max.unwrap_or(usize::MAX);
    let b_max = b.max.unwrap_or(usize::MAX);
    a.min <= b_max && b.min <= a_max
}

/// The arity an arrow signature admits — the same mapping the call-site arity check
/// applies to a declared sig.
fn sig_arity(sig: &Sig) -> Arity {
    if sig.rest.is_some() {
        Arity::at_least(sig.params.len())
    } else if sig.optional.is_empty() {
        Arity::exact(sig.params.len())
    } else {
        Arity::range(sig.params.len(), sig.params.len() + sig.optional.len())
    }
}

/// Validate this file's hand-written `(sig …)` declarations (Pass 2.85) — see the
/// call site for why an unreadable annotation is worse than no annotation.
///
/// Reads the *un-expanded* forms, which is where a hand-written declaration is
/// legible; a macro-emitted sig (`defrecord`'s) is machine-built and deliberately
/// not second-guessed here.
fn check_sig_declarations(
    heap: &Heap,
    forms: &[Value],
    file_ns: Option<&str>,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    for &form in forms {
        let Some((name, ty_form)) = annot::sig_decl_parts(heap, form) else {
            continue;
        };
        let spelling = value::symbol_name(name);
        let pos = heap.form_pos_only(form);
        // 1. Is the type-expression readable at all?
        if let Some(problem) = annot::type_expr_problem(heap, ty_form) {
            out.push((pos, format!("sig {spelling}: {problem}")));
            continue; // the rest reads the parsed sig, which doesn't exist
        }
        let qualified = qualify_decl_name(ctx, file_ns, name);
        // 2. Does it annotate anything? A sig for a name this file never defines (and
        //    that isn't bound anywhere in the image) annotates nothing — usually a
        //    rename that moved the `defn` and left the declaration behind.
        if walk::is_unbound(heap, ctx, name) && walk::is_unbound(heap, ctx, qualified) {
            out.push((
                pos,
                format!("sig {spelling}: nothing named `{spelling}` is defined here"),
            ));
            continue;
        }
        // 3. Does its arity agree with the definition's? Only a *disjoint* pair is
        //    provably wrong — see `arities_overlap`.
        if let (Some((_, sig)), Some(def_arity)) = (
            annot::parse_sig_decl(heap, form),
            ctx.file_arity(qualified).or_else(|| ctx.file_arity(name)),
        ) {
            let declared = sig_arity(&sig);
            if !arities_overlap(declared, def_arity) {
                out.push((
                    pos,
                    format!(
                        "sig {spelling}: declares {} argument(s) but the definition takes {}",
                        sigs::arity_str(declared),
                        sigs::arity_str(def_arity),
                    ),
                ));
            }
        }
    }
}

/// Recover `(sig name type)` forms from the *expanded* tree. Each `(sig …)` — hand-
/// written or emitted by a macro like `defrecord` — lowers to `(%register-sig 'name
/// 'type)`; this walks into `(do …)` blocks (what those macros wrap their output in)
/// and, for each `%register-sig`, rebuilds the equivalent `(sig name type)` form so
/// [`register_declared_sig`] can parse it with the ordinary sig parsers. Building the
/// form needs `&mut Heap`; GC is blocked for the whole check, so the pushed handles
/// stay live.
fn collect_register_sig_forms(heap: &mut Heap, form: Value, out: &mut Vec<Value>) {
    // Recurses through nested `(do (do …))`; a deep-but-legal chain would blow
    // the native stack (a SIGSEGV `catch_unwind` can't catch — the sibling of
    // the walk.rs/recursion.rs hardening). Grow the stack in heap-backed
    // segments like the rest.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_register_sig_forms_inner(heap, form, out)
    })
}

fn collect_register_sig_forms_inner(heap: &mut Heap, form: Value, out: &mut Vec<Value>) {
    let Ok(items) = heap.list_to_vec(form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    if crate::core::value::symbol_is(head, kw::DO) {
        for &it in &items[1..] {
            collect_register_sig_forms(heap, it, out);
        }
        return;
    }
    if crate::core::value::symbol_is(head, "%register-sig") && items.len() == 3 {
        // items[1] = (quote name), items[2] = (quote type)
        if let (Some(name), Some(ty)) = (unwrap_quote(heap, items[1]), unwrap_quote(heap, items[2]))
        {
            let sig_head = crate::core::value::sym("sig");
            let rebuilt = heap.list(vec![sig_head, name, ty]);
            out.push(rebuilt);
        }
    }
}

/// The inner form of `(quote X)` → `X`; `None` for anything else.
fn unwrap_quote(heap: &Heap, form: Value) -> Option<Value> {
    let items = heap.list_to_vec(form).ok()?;
    match items.as_slice() {
        [Value::Sym(h), inner] if crate::core::value::symbol_is(*h, kw::QUOTE) => Some(*inner),
        _ => None,
    }
}

/// A namespace header — `(defmodule …)` (checked on the *un-expanded* form, before
/// its `(:use …)` clauses lower away). The checker evaluates it so the header's
/// `(require …)`/`%refer`/`%in-ns` run — populating the import table — and a
/// `(:use …)`-imported name then resolves instead of looking unbound.
fn is_ns_header(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, kw::DEFMODULE);
        }
    }
    false
}

/// Parse the `(defmodule … (:use mod) …)` header from the *unexpanded* `forms`
/// and return the module names explicitly listed in `:use` clauses.
fn extract_use_module_names(heap: &Heap, forms: &[Value]) -> Vec<String> {
    extract_clause_modules(heap, forms, &["use"])
}

/// Every module named in an *import* clause of the header — `(:use mod)`,
/// `(:use-internals mod)`, and `(:alias mod [:as short])`, each of which **loads** `mod`
/// (an `:alias` `(require …)`s its target, then adds the `short/` prefix; references via
/// the alias macro-expand to `mod/…`). Used for the KI-17 reachability set — a module
/// reached by any of these is genuinely required. Kept separate from
/// [`extract_use_module_names`], whose `:use`-only view backs the unused-import lint.
fn extract_import_module_names(heap: &Heap, forms: &[Value]) -> Vec<String> {
    extract_clause_modules(heap, forms, &["use", "use-internals", "alias"])
}

/// The module names listed by any header clause whose keyword is in `keywords`
/// (e.g. `:use` / `:use-internals`), read from the *unexpanded* `(defmodule …)` form.
fn extract_clause_modules(heap: &Heap, forms: &[Value], keywords: &[&str]) -> Vec<String> {
    for &form in forms {
        if !is_ns_header(heap, form) {
            continue;
        }
        // (defmodule name clause...)
        let Value::Pair(p) = form else { continue };
        let (_, rest) = heap.pair(p);
        let Value::Pair(r) = rest else { continue };
        let (_, clauses) = heap.pair(r); // skip the module name

        let mut result = Vec::new();
        let mut cur = clauses;
        while let Value::Pair(p) = cur {
            let (clause, next) = heap.pair(p);
            // Each (:use mod …) / (:use-internals mod …) clause starts with its keyword.
            if let Some(items) = list_items(heap, clause) {
                if let Some(Value::Keyword(kw_sym)) = items.first() {
                    if keywords.iter().any(|k| value::symbol_is(*kw_sym, k)) {
                        if let Some(&Value::Sym(mod_sym)) = items.get(1) {
                            result.push(value::symbol_name(mod_sym).to_string());
                        }
                    }
                }
            }
            cur = next;
        }
        return result;
    }
    Vec::new()
}

/// The module name a top-level `(require …)` targets, if any — `(require 'mod)`,
/// `(require "mod")`, or `(require 'mod :as m)`. `None` for a non-require form or a
/// dynamic require whose module argument isn't a literal symbol/string (harmless: an
/// un-added prefix can only *add* a warning, so a dynamic require that this misses is
/// caught by the `raw_qualified` guard / the sweep, never a silent unsoundness the
/// other way).
fn require_target(heap: &Heap, form: Value) -> Option<String> {
    let items = list_items(heap, form)?;
    match items.first() {
        Some(&Value::Sym(h)) if value::symbol_is(h, "require-one") => {}
        _ => return None,
    }
    match *items.get(1)? {
        // `'mod` reads as `(quote mod)`.
        arg @ Value::Pair(_) => {
            let inner = list_items(heap, arg)?;
            match (inner.first(), inner.get(1)) {
                (Some(&Value::Sym(q)), Some(&Value::Sym(m))) if value::symbol_is(q, "quote") => {
                    Some(value::symbol_name(m))
                }
                _ => None,
            }
        }
        Value::Sym(m) => Some(value::symbol_name(m)),
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    }
}

/// The KI-17 reachability set — module prefixes the file makes reachable *itself*:
/// every `(:use M)` in its header, every `(require 'M)` **anywhere** in the file
/// (including one nested in a function body — `project.blsp` requires `test`/`package`
/// lazily inside the functions that use them, to keep startup lean), and its own
/// namespace. A file that requires `M` *somewhere* is treated as reaching `M` for the
/// whole file — an over-approximation, which is the sound direction (it can only
/// *suppress* a warning, never invent one). Direct requires only, not their transitive
/// closure; matching the discipline that a file `require`s what it names. See
/// [`Ctx::required_mods`] and `walk::unrequired_module`.
fn collect_required_modules(
    heap: &Heap,
    forms: &[Value],
    file_ns: Option<Symbol>,
) -> HashSet<String> {
    let mut mods: HashSet<String> = extract_import_module_names(heap, forms)
        .into_iter()
        .collect();
    if let Some(ns) = file_ns {
        mods.insert(value::symbol_name(ns));
    }
    // Every module the file itself declares (ADR-223: a file may open more than one
    // `(defmodule …)`) is trivially "required" — an intra-file qualified reference to a
    // co-located sibling module must not read as an unrequired-module use. `file_ns` above
    // is only the first; add the rest (the caller roots every entry, so the bare names here
    // gain their rooted form too).
    for m in crate::eval::macros::file_modules(heap, forms) {
        mods.insert(value::symbol_name(m));
    }
    for &form in forms {
        collect_require_targets(heap, form, &mut mods);
    }
    mods
}

/// A file's own module name (from its `(defmodule …)` header, or `None`) and the module
/// names it **directly** pulls in — `(:use …)` / `(:use-internals …)` clauses plus every
/// `(require 'M)` anywhere in the file. The edge list the whole-project driver
/// (`std/tool/project.blsp`) closes transitively into each file's KI-17 reachability set,
/// then feeds back to [`check_file_ext`]. Own ns is excluded from `requires`.
pub fn module_direct_requires(heap: &Heap, forms: &[Value]) -> (Option<String>, Vec<String>) {
    let file_ns = crate::eval::macros::file_ns(heap, forms);
    let own = file_ns.map(value::symbol_name);
    let mut deps: HashSet<String> = extract_import_module_names(heap, forms)
        .into_iter()
        .collect();
    for &form in forms {
        collect_require_targets(heap, form, &mut deps);
    }
    if let Some(ref n) = own {
        deps.remove(n);
    }
    let mut deps: Vec<String> = deps.into_iter().collect();
    deps.sort(); // deterministic order (no Date/random in the checker)
    (own, deps)
}

/// Add every `(require 'M)` target reachable in `form` — descending through list,
/// vector, and map structure so a `require` nested in any function body is found.
fn collect_require_targets(heap: &Heap, form: Value, out: &mut HashSet<String>) {
    // Guard the deep-form recursion the same way `count_defs` does — a pathologically
    // nested source must not overflow the checker's stack (tests/…deep_forms).
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        if let Some(m) = require_target(heap, form) {
            out.insert(m);
        }
        match form {
            Value::Pair(pid) => {
                let (car, cdr) = heap.pair(pid);
                collect_require_targets(heap, car, out);
                collect_require_targets(heap, cdr, out);
            }
            Value::Vector(vid) => {
                for v in heap.vector(vid).iter() {
                    collect_require_targets(heap, *v, out);
                }
            }
            Value::Map(mid) => {
                for (k, v) in heap.map_entries(mid).iter() {
                    collect_require_targets(heap, *k, out);
                    collect_require_targets(heap, *v, out);
                }
            }
            _ => {}
        }
    })
}

/// Every qualified symbol *name* (`"mod/name"`, module segment non-empty) appearing
/// anywhere in the un-expanded `forms` — the user-written references. Over-approximate
/// (includes quoted data and binder positions), which is sound for the KI-17 lint: it
/// only *permits* a warning the walk independently decides to emit (and the walk already
/// excludes quotes/binders), so a name present only after macro expansion is treated as
/// macro-injected and never flagged.
fn collect_raw_qualified(heap: &Heap, forms: &[Value]) -> HashSet<String> {
    collect_all_syms(heap, forms)
        .into_iter()
        .filter_map(|s| {
            let n = value::symbol_name(s);
            match n.rfind('/') {
                Some(slash) if slash > 0 => Some(n),
                _ => None,
            }
        })
        .collect()
}

/// Populate the current file's import table from a `(defmodule … (:use …)/(:alias …))`
/// header WITHOUT evaling/compiling the header. In a whole-project check every module is
/// already loaded (`project--ensure-loaded`), so `check_file` can set up imports directly
/// instead of re-macroexpanding + re-evaling + re-compiling every file's header — the eval
/// path's per-file `provide`/`require`/`%refer` all-globals scans + `*module-docs*` rebind +
/// compile made a whole-project check O(files²) (see docs/devlog.md 2026-07-03).
///
/// Mirrors `defmodule`'s `(:use)`/`(:alias)` expansion (std/prelude.blsp) + the `%refer`/
/// `%alias` builtins exactly: `(:use mod)` refers mod's public (non-`--`) names bare;
/// `(:use mod :only [a b])` / `:refer` refers just those; `(:alias mod [:as short])` adds a
/// `short/` → `mod` prefix alias. A used module that isn't loaded (a bare-file check outside a
/// project) is `require`d first — rare, and correctness there beats the O(files²) speed win.
fn setup_check_imports(heap: &mut Heap, header: Value) {
    // PASS A — parse the clauses into GC-STABLE data (`Symbol`s are Copy `u32`) with NO
    // eval, so nothing LOCAL is held across the `require` eval in pass B (which can collect
    // and relocate handles). Holding a parsed `Value` across that eval was a use-after-GC.
    enum Clause {
        Use(Symbol, Option<Vec<Symbol>>), // (module, Some(:only subset) | None = all public)
        UseExcept(Symbol, Vec<Symbol>),   // (:use mod :exclude [names]) — all public minus names
        Alias(Symbol, Symbol),            // (short-prefix-name, module)
        UseInternals(Symbol),             // (:use-internals mod) — ADR-146 grant
    }
    let mut clauses: Vec<Clause> = Vec::new();
    {
        let Some(items) = list_items(heap, header) else {
            return;
        };
        // items = [defmodule, mod-name, doc?, clause...]; clauses follow name + optional doc.
        let first_clause = if matches!(items.get(2), Some(Value::Str(_))) {
            3
        } else {
            2
        };
        for clause in items.iter().skip(first_clause) {
            let Some(citems) = list_items(heap, *clause) else {
                continue;
            };
            let Some(Value::Keyword(kw_sym)) = citems.first() else {
                continue;
            };
            if value::symbol_is(*kw_sym, "use") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // `:only [a b]` → refer just those; `:exclude [a b]` → all public minus
                // those; no marker → all public. Mirrors the runtime `defmodule--use-clause`
                // vocabulary exactly (`:refer` is not a marker — the runtime rejects it, and
                // the checker's compile pass surfaces that as a "does not compile" diagnostic).
                let marker = citems.get(2);
                let is_only =
                    matches!(marker, Some(Value::Keyword(m)) if value::symbol_is(*m, "only"));
                let is_exclude =
                    matches!(marker, Some(Value::Keyword(m)) if value::symbol_is(*m, "exclude"));
                if is_only || is_exclude {
                    // `:only`/`:exclude [a b]` uses a VECTOR literal (also accepts a list), so
                    // read it with `seq_items` (vector + list) — NOT `list_items` (cons-only),
                    // which would miss a vector. Read-only, no alloc → no GC, so holding
                    // `items`/`citems` across it is safe.
                    let names: Vec<Symbol> = citems
                        .get(3)
                        .and_then(|sub| heap.seq_items(*sub).ok())
                        .map(|ns| {
                            ns.iter()
                                .filter_map(|n| match n {
                                    Value::Sym(s) => Some(*s),
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if is_only {
                        clauses.push(Clause::Use(mod_sym, Some(names)));
                    } else {
                        clauses.push(Clause::UseExcept(mod_sym, names));
                    }
                } else {
                    clauses.push(Clause::Use(mod_sym, None));
                }
            } else if value::symbol_is(*kw_sym, "use-internals") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // A grant is also a use (publics refer bare), plus the ADR-146
                // internals key the privacy walk consults.
                clauses.push(Clause::Use(mod_sym, None));
                clauses.push(Clause::UseInternals(mod_sym));
            } else if value::symbol_is(*kw_sym, "alias") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // `(:alias mod [:as short])`; `short` defaults to mod's last `/`-segment.
                let short = match (citems.get(2), citems.get(3)) {
                    (Some(Value::Keyword(m)), Some(Value::Sym(s)))
                        if value::symbol_is(*m, "as") =>
                    {
                        *s
                    }
                    _ => {
                        let mn = value::symbol_name(mod_sym);
                        value::intern(mn.rsplit('/').next().unwrap_or(&mn))
                    }
                };
                clauses.push(Clause::Alias(short, mod_sym));
            }
        }
    }
    // PASS B — apply. Only `Symbol`s (Copy, GC-stable) are held, so the `require` eval below
    // (standalone-file path) can safely collect. `module_public_exports` re-reads globals
    // fresh; `add_import` takes `Symbol`s.
    // Load `mod_sym` unless it's already loaded — the standalone path; in a whole-project
    // check every module is loaded up front, so this is a no-op. Advisory: any load error
    // is swallowed (the checker never gates on a missing module).
    //
    // "Already loaded" is the FEATURE registry, not "some `mod/*` global exists". The
    // latter was the bug: a std module whose namespace is *also* a kernel-primitive
    // namespace — `file`, with its 18 `file/…` primitives — looked loaded before its
    // `.blsp` had ever been read, so `(:use file)` imported the primitives only and every
    // Brood-level name in it (`walk-files`, `read-lines`, `regular?`) was reported unbound
    // in a single-file `brood --check` (the whole-project path was unaffected: it loads
    // every module first). `*features*` is exactly what the runtime `require-one` consults,
    // so the checker's notion of loaded now matches the runtime's.
    //
    // Erring toward "not loaded" is free: `require-one` is idempotent, so a spurious call
    // costs a map lookup; erring toward "loaded" is what produced the false positives.
    fn ensure_loaded(heap: &mut Heap, mod_sym: Symbol) {
        if feature_loaded(heap, &value::symbol_name(mod_sym)) {
            return;
        }
        let quoted = heap.list(vec![
            Value::Sym(value::intern("quote")),
            Value::Sym(mod_sym),
        ]);
        let form = heap.list(vec![Value::Sym(value::intern("require-one")), quoted]);
        let root = heap.global();
        let _ = crate::eval::eval(heap, form, root);
    }
    for clause in clauses {
        match clause {
            Clause::Use(mod_sym, subset) => {
                // Package-rooted namespaces (ADR-070): an intra-package `(:use b)` — in a
                // dependency, or in the root project under its `:name` — roots to `pkg/b`.
                // The runtime `%refer` target roots via `%root-module-name`; the checker
                // must root the same way here or it scans `b/*` for exports that actually
                // live under `pkg/b/*` and flags every imported name unbound. External /
                // std / already-qualified names are left unchanged.
                let mod_sym = heap.root_module_name(mod_sym);
                let mod_name = value::symbol_name(mod_sym);
                let prefix = format!("{}/", mod_name);
                ensure_loaded(heap, mod_sym);
                match subset {
                    Some(names) => {
                        // Mirror the runtime `%refer`: a module-private name in `:only`
                        // needs an internals grant, else the runtime refuses the load —
                        // so the checker must NOT treat it as a bound import either
                        // (otherwise it resolves-as-bound a name whose file won't load).
                        // `ensure_loaded` ran above, so the privacy record is exact.
                        let granted = heap
                            .import_of(crate::eval::macros::internals_grant_key(&mod_name))
                            .is_some();
                        for bare in names {
                            let bare_name = value::symbol_name(bare);
                            let qual = value::intern(&format!("{}/{}", mod_name, bare_name));
                            if heap.is_private(qual) && !granted {
                                continue;
                            }
                            heap.add_import_lazy(bare, qual);
                        }
                    }
                    None => {
                        for (bare, qual) in deps::obs_module_exports(heap, &prefix) {
                            heap.add_import_lazy(bare, qual);
                        }
                    }
                }
            }
            Clause::UseExcept(mod_sym, excluded) => {
                let mod_sym = heap.root_module_name(mod_sym); // ADR-070, as in Clause::Use
                let mod_name = value::symbol_name(mod_sym);
                let prefix = format!("{}/", mod_name);
                ensure_loaded(heap, mod_sym);
                let excluded: std::collections::HashSet<Symbol> = excluded.into_iter().collect();
                for (bare, qual) in deps::obs_module_exports(heap, &prefix) {
                    if !excluded.contains(&bare) {
                        heap.add_import_lazy(bare, qual);
                    }
                }
            }
            Clause::UseInternals(mod_sym) => {
                let mod_sym = heap.root_module_name(mod_sym); // ADR-070, as in Clause::Use
                let key = crate::eval::macros::internals_grant_key(&value::symbol_name(mod_sym));
                heap.add_import(key, mod_sym);
            }
            Clause::Alias(short, mod_sym) => {
                // Root the alias TARGET (ADR-070) so `short/name` resolves to `pkg/mod/name`;
                // the local `short` prefix is unchanged.
                let mod_sym = heap.root_module_name(mod_sym);
                let key = value::intern(&format!("{}/", value::symbol_name(short)));
                heap.add_import(key, mod_sym);
            }
        }
    }
}

/// The **type signature** of the callable `sym` resolves to — declared, curated, or
/// inferred (all of `sigs::sig_of`'s sources) — rendered as its arrow string, e.g.
/// `(int -> int)` or `(fn seqable -> seqable)`. `None` for a non-callable, an unknown
/// name, or one whose signature can't be pinned. The tooling-facing view of the
/// inferencer (LSP hover, docs); reads only, never gates. The per-file inference memo is
/// cleared first so a re-edited/reloaded function re-infers rather than showing a stale sig.
pub fn signature_string(heap: &Heap, sym: Symbol) -> Option<String> {
    sigs::clear_sig_memo();
    sigs::sig_of(heap, sym).map(|s| s.to_string())
}

/// Check one form, returning a warning per provable misuse. Empty when nothing is
/// provably wrong (which includes "not enough static info").
pub fn check_form(heap: &Heap, form: Value) -> Vec<String> {
    check_located(heap, form)
        .into_iter()
        .map(|(_, msg)| msg)
        .collect()
}

/// Like [`check_form`], but each warning carries the source `Pos` of the call it
/// was found in (when known) — for `file:line:col:` diagnostics from `brood
/// --check` / `nest check`. The position is the *call form*'s, recorded by the
/// reader; an unrecorded form (e.g. one a macro synthesised) yields `None`.
pub fn check_located(heap: &Heap, form: Value) -> Vec<(Option<Pos>, String)> {
    // This ad-hoc single-form path never populates the sealed-ability-as-a-type table
    // (that's `check_file`'s job). Clear it so a prior `check_file` on this thread can't
    // leak a stale table into a `(check 'form)` — defensive: an ability type is sound
    // regardless, but an empty table here is unambiguously so.
    annot::clear_ability_types();
    let mut out = Vec::new();
    check_into(heap, form, &Ctx::default(), &mut out);
    out
}

/// Check a sequence of top-level forms together, threading file-local
/// definitions across them so a `(defn foo …)` at the top isn't flagged when
/// a later form calls `foo`. This is the entry point for `brood --check
/// <file>` / `nest check`.
///
/// Each form is **macro-expanded first** (like the `(check 'form)` builtin),
/// so threading macros (`->`/`->>`), pattern syntax (`match`), test framework
/// wrappers (`test`/`describe`/…), and any user macro that rearranges code
/// are checked against their *expanded* shape — not the surface syntax that
/// would otherwise mistake `(map inc)` inside `(->> xs (map inc))` for a
/// 1-arg call. Source positions survive expansion where the macro rebuilds
/// through `rebuild_list` (the common case); positions on macro-introduced
/// new code are absent.
///
/// File-local def names are accumulated by a **recursive** scan over the
/// expanded forms, so a `(defn foo …)` nested inside a macro body
/// (e.g. inside `(test … (defn foo …) …)`) still shields a later `(foo …)`
/// — `def`s define globally in Brood regardless of nesting position
/// (`docs/language.md`).
///
/// A form whose macroexpansion fails (a malformed macro call) falls back to
/// its un-expanded shape — the eval path will surface the same parse-time
/// error later anyway, so the checker just stays quiet there.
pub fn check_file(heap: &mut Heap, forms: &[Value]) -> Vec<(Option<Pos>, String)> {
    check_file_ext(heap, forms, &[])
}

/// The checker-inferred [`Ty`] of item `arg_index` (0 = the head) of the **call form
/// recorded at 1-based reader position `line:col`** in `forms` — the position-keyed
/// type query behind the LSP's record-field completion (`docs/lsp.md`). Runs the full
/// [`check_file`] analysis with a capture hook armed and discards the diagnostics, so
/// everything the walk knows is in force at the capture point: the ctor `sig`s a
/// same-file `defrecord` emits, Gap A value types, same-file function returns, and the
/// `let`/param bindings + guard narrowings of the enclosing scope.
///
/// Keyed by the *call form's* position (the reader records a list's `Pos` at its
/// opening paren) rather than the argument's own, because the interesting argument is
/// typically a bare symbol — and the form-pos table is pair-keyed, so a symbol carries
/// no position. `None` when the form isn't found, the item is missing, or its type
/// can't be pinned — the caller degrades to no candidates, never a wrong list.
pub fn arg_ty_at(
    heap: &mut Heap,
    forms: &[Value],
    line: u32,
    col: u32,
    arg_index: usize,
) -> Option<Ty> {
    walk::arm_arg_ty_query(line, col, arg_index);
    let _ = check_file(heap, forms);
    walk::take_arg_ty_query()
}

/// The checker's static type for a **closed expression** — the entry point a test or
/// tool needs to ask "what does the checker think this value is?" without running a
/// whole file check. For a literal the answer is exact, which is what makes it usable
/// as one side of an agreement check against the runtime contract
/// (`tests/type_grammar_agreement.rs`).
pub fn expr_ty_of(heap: &Heap, form: Value) -> Option<Ty> {
    infer::expr_ty(heap, form, &Ctx::default())
}

/// What a file's functions are, as the checker sees them — the data behind the LSP's
/// **effective-type inlay hints** (`docs/lsp.md`).
#[derive(Clone, Debug)]
pub struct FnSignature {
    /// The name as written at the def site (unqualified).
    pub name: String,
    /// Its signature: what the author *declared*, else what the checker inferred.
    pub sig: Sig,
    /// True when a `(sig …)` states this — in which case the signature is already on
    /// screen, and a hint repeating it is noise.
    pub declared: bool,
}

/// Every function a file defines, with its **effective** signature.
///
/// This is the question a buffer wants answered and `hover` could not: hover reads the
/// *loaded* image, and a file being edited is not loaded. The checker has inferred
/// same-file functions from their FORMS since ADR-188/190 (and per-clause since
/// ADR-261) precisely because it cannot load them either — so the answer already
/// exists, one pass away, and this exposes it.
///
/// Runs the full [`check_file`] analysis with a capture armed and discards the
/// diagnostics, exactly as [`arg_ty_at`] does: one code path, so a hint can never
/// describe a different inference than the warnings do.
pub fn file_signatures(heap: &mut Heap, forms: &[Value]) -> Vec<FnSignature> {
    arm_signature_capture();
    let _ = check_file(heap, forms);
    take_signature_capture()
}

thread_local! {
    /// Armed by [`file_signatures`]; filled once the passes have built the `Ctx`.
    static SIGNATURE_CAPTURE: std::cell::RefCell<Option<Vec<FnSignature>>> =
        const { std::cell::RefCell::new(None) };
}

fn arm_signature_capture() {
    SIGNATURE_CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
}

fn take_signature_capture() -> Vec<FnSignature> {
    SIGNATURE_CAPTURE
        .with(|c| c.borrow_mut().take())
        .unwrap_or_default()
}

/// Record each `defn`'s effective signature, if a capture is armed. Called once the
/// inference passes have run, so a declared sig, an inferred single arm and an
/// inferred multi-arm set are all visible.
fn capture_file_signatures(heap: &Heap, expanded: &[Value], ctx: &Ctx) {
    SIGNATURE_CAPTURE.with(|c| {
        let mut slot = c.borrow_mut();
        let Some(out) = slot.as_mut() else {
            return; // not armed — the ordinary checking path pays nothing
        };
        for form in top_level_defs(heap, expanded) {
            let Some((name, rhs)) = def_name_and_value(heap, form) else {
                continue;
            };
            // Only functions: a `(def x 5)` has a value type, not a signature. And
            // never a macro's generated temporary — nobody can write a `(sig …)` for a
            // name they cannot refer to.
            if !walk::is_fn_value_form(heap, rhs)
                || ctx.is_file_macro(name)
                || value::is_gensym(name)
            {
                continue;
            }
            let (sig, declared) = match ctx.declared_sig(name) {
                Some(sig) => (sig, true),
                None => match inferred_signature(ctx, name) {
                    // An inferred signature is a *fact about types*; it carries no
                    // obligation to describe the definition's shape, and for a function
                    // whose params the checker could not type at all it is bare
                    // `(-> ret)`. Written into a file that way it declares a NULLARY
                    // function, which ADR-259's Pass 2.85 then rejects as contradicting
                    // its `defn` — so a suggestion must never be offered in that state.
                    Some(sig) => (reconcile_arity(heap, rhs, sig), false),
                    None => continue,
                },
            };
            out.push(FnSignature {
                name: value::symbol_name(name),
                sig,
                declared,
            });
        }
    });
}

/// The most informative inferred signature for `name`.
///
/// A **multi-arm** function has one signature per arm (ADR-261) and, separately, a
/// params-less sig carrying the union of the arms' returns — because a guarded
/// multi-clause `defn` lowers to one variadic `fn`, so the per-position inference
/// declines while the return inference does not. Neither half alone is worth showing:
/// the first is `(string) -> any`, the second `() -> 1 | 2`. Take the first arm's
/// parameters and that union return, which is a sound over-approximation of what the
/// arm returns and the shape a reader is actually looking at.
fn inferred_signature(ctx: &Ctx, name: Symbol) -> Option<Sig> {
    let single = ctx.inferred_fn_sig(name);
    let Some(arm) = ctx.inferred_overload(name).and_then(|a| a.first().cloned()) else {
        return single;
    };
    let ret = match &single {
        Some(s) if s.params.is_empty() => s.ret.clone(),
        _ => arm.ret.clone(),
    };
    Some(Sig::new(arm.params, ret))
}

/// [`check_file`] with an explicit **KI-17 reachability set** — the file's *transitive*
/// require-closure (module names), computed by the whole-project driver
/// (`std/tool/project.blsp`, which alone sees every file's header). Unioned with the
/// file's own direct requires (which `check_file_ext` derives from `forms`), it becomes
/// [`Ctx::required_mods`]: a user-written `mod/name` whose `mod` is outside the closure
/// resolves only by load-order luck and is flagged. Pass `&[]` for the single-file /
/// LSP path, where the un-required module simply isn't loaded and the lint is inert.
pub fn check_file_ext(
    heap: &mut Heap,
    forms: &[Value],
    extra_required: &[String],
) -> Vec<(Option<Pos>, String)> {
    check_file_mode(heap, forms, extra_required, super::strict_checking())
}

/// [`check_file_ext`] with the strict switch passed explicitly rather than read from the
/// process-wide setting — what a test uses, so strictness never leaks between tests
/// running in parallel.
pub fn check_file_mode(
    heap: &mut Heap,
    forms: &[Value],
    extra_required: &[String],
    strict: bool,
) -> Vec<(Option<Pos>, String)> {
    let mut out = Vec::new();
    // Block the copying GC for the whole check: this fn holds LOCAL handles in
    // Rust `Vec`s (`forms`/`expanded`) *across* the `eval` of `(require …)` forms
    // below, and a collection there would relocate them (copying moves objects),
    // leaving the Vec copies stale. Bumping `GC_BLOCK` makes those inner evals run
    // at depth ≥ 2 so the outermost-eval safepoint never fires mid-check — the
    // same guard `macroexpand_all` uses for its partially-built forms. The
    // checker's allocations are bounded (one file) and reclaimed at the next real
    // safepoint after it returns. See ADR-054 / `docs/memory-review.md`.
    let _gc_block = crate::process::GcBlockGuard::enter();
    // Fresh per-pass signature-inference memo: this file's inferred sigs must not leak
    // into another file, nor (in the long-lived LSP) survive a source edit (KI-13).
    sigs::clear_sig_memo();
    // Fresh per-file sealed-ability-as-a-type table (ADR-181); repopulated below once the
    // expanded tree is built, so a bare `(sig f (Shape -> …))` resolves. Cleared here so a
    // panic mid-check can't leak one file's abilities into the next.
    annot::clear_ability_types();
    // Pass 1: macroexpand each form (recording the expanded shape we'll also
    // walk in pass 2). A macroexpand failure isn't this pass's job to report,
    // so we fall back to the un-expanded form silently.
    //
    // When a top-level form is `(require 'mod …)`, also *evaluate* it so the
    // module's macros and globals become resolvable for the rest of the file.
    // Otherwise the next form using a macro the module brought in
    // (`defserver`, `cast`, `!`, etc. from `std/proc/gen.blsp`) would expand as
    // an un-known head and trip the unbound-symbol diagnostic. `require` is
    // idempotent (it checks `*features*`), so a later real run re-evaluating
    // the same form is a no-op. Failures are swallowed: the checker is
    // advisory and shouldn't gate on a missing module.
    let root = heap.global();
    // Namespace-aware checking (ADR-065): if the file declares `(ns foo)`, set the
    // compile namespace + forward-ref pre-scan so pass 1's resolve qualifies both
    // definition heads and references to `foo/…` — otherwise every qualified
    // reference would look unbound. Restored before returning.
    // ROOTED (ADR-070): `%in-ns` roots what `(defmodule tutor)` declares, so at run time the
    // namespace is `bedit/tutor`. The checker has to set the SAME rooted namespace, or every
    // comparison against it disagrees with the runtime — most visibly the `--` privacy rule,
    // which then reads a module's reference to its own helper as a foreign private access.
    let file_ns = crate::eval::macros::file_ns(heap, forms).map(|ns| heap.root_module_name(ns));
    let prev_ns = heap.set_compile_ns(file_ns);
    // Region model (ADR-223): a file may declare more than one `(defmodule …)`. Install the
    // per-module forward-ref pre-scan and start the active set on the FIRST module's region;
    // the pass-1 loop below switches compile-ns + region at each subsequent `defmodule`
    // (the checker doesn't `eval` `%in-ns`, so it must mirror that switch itself). A
    // single-module file has one region equal to the old whole-file scan.
    let regions = if file_ns.is_some() {
        crate::eval::macros::scan_regions(heap, forms)
    } else {
        std::collections::HashMap::new()
    };
    let first_known = crate::eval::macros::file_modules(heap, forms)
        .first()
        .and_then(|m| regions.get(m).cloned())
        .unwrap_or_default();
    let prev_known = heap.set_ns_known_names(first_known);
    let prev_by_module = heap.set_ns_known_by_module(regions);
    // Imports start empty; a `(:use …)` in the header populates them during pass 1
    // (its `(require …)`/`%refer` is evaluated like any other header form).
    let prev_imports = heap.set_imports(std::collections::HashMap::new());
    // Root the input forms and the expanding-into vec across the loop:
    // each iteration may call `eval` on a `(require …)`, which runs a
    // GC safepoint at outermost depth — any LOCAL `Value` held only in
    // a Rust local would be swept. The `roots_len`/`truncate_roots`
    // pairing is balanced even if a panic unwinds through here (a future
    // `panic = abort` wouldn't need this, but today's `unwind` would
    // leak roots otherwise).
    let roots_base = heap.roots_len();
    // Panic containment (host-panic hardening, 2026-07-23): the checker is
    // advisory and runs inside long-lived hosts (brood-lsp, `nest check`, the
    // REPL's background check) — an internal panic must not tear the host
    // down, and must not leave the compile-ns / known-names / imports / GC
    // roots captured above poisoned for the next check. The whole analysis
    // runs under `catch_unwind`; the restores below run on BOTH paths, and a
    // panic degrades to one "checker internal error" diagnostic.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for &f in forms {
            heap.push_root(f);
        }
        let n = forms.len();
        let mut expanded: Vec<Value> = Vec::with_capacity(n);
        for j in 0..n {
            // Re-read the (relocated) form from the root stack, NOT the `forms` slice:
            // an earlier iteration's `(require …)` `eval` can collect at any depth
            // (ADR-061) and relocate it, so the slice's copy is stale by now.
            let f = heap.root_at(roots_base + j);
            // Compile pass: macroexpand then namespace-resolve, so the analysed tree
            // matches what `eval` will see (qualified defs + references). A compile
            // ERROR is a real diagnostic now (ADR-146 module-privacy violations
            // surface here — the file would fail to load), not silently swallowed;
            // the un-expanded form still feeds the rest of the walk so the other
            // lints run.
            let exp = match crate::eval::macros::compile(heap, f, root) {
                Ok(e) => e,
                // Re-read the relocated form: `compile` now infers `(require …)` from
                // qualified references (ADR-227 follow-up), and that module load can
                // collect at any depth, so `f` captured above is stale on the error path.
                Err(err) => {
                    // A macro's own `(error …)` during expansion carries no position; the
                    // form it was expanding does. Report there rather than nowhere.
                    out.push((
                        err.pos.or_else(|| heap.form_pos_only(forms[j])),
                        format!("does not compile: {}", err.message),
                    ));
                    heap.root_at(roots_base + j)
                }
            };
            // Root the just-built expansion *before* possibly triggering a
            // collect via `eval`; otherwise this LOCAL handle dies between
            // here and the next iteration's macroexpand.
            heap.push_root(exp);
            expanded.push(exp);
            // `compile` above can collect (it infers `(require …)` from qualified
            // references, ADR-227 follow-up), relocating `f` — re-read the live handle
            // from the root stack before the header checks that dereference it.
            let f = heap.root_at(roots_base + j);
            // Make the file's imports + required modules resolvable for the rest of the walk.
            // For a `(defmodule … (:use …))` header, populate the import table DIRECTLY from its
            // clauses (`setup_check_imports`) instead of evaling the expanded header — the eval
            // re-macroexpands + re-compiles it and runs per-file O(globals) `provide`/`require`/
            // `%refer` scans, which made a whole-project check O(files²). A standalone
            // `(require …)` form (not a header) is still evaluated so its macros/globals resolve.
            if is_ns_header(heap, f) {
                // Region model (ADR-223): a later `(defmodule …)` opens a new region — switch
                // the checker's compile-ns + forward-ref set to it, exactly as `%in-ns` does
                // at run time, so this module's defs/refs qualify to it and not the first.
                if let Some(m) = crate::eval::macros::defmodule_form_name(heap, f) {
                    let rooted = heap.root_module_name(m);
                    heap.set_compile_ns(Some(rooted));
                    heap.activate_ns_region(m);
                }
                setup_check_imports(heap, f);
            } else if is_require_form(heap, exp) {
                let _ = crate::eval::eval(heap, exp, root);
            }
        }
        // A pass-1 `(require …)` `eval` can collect at ANY depth (ADR-061), which
        // relocates the rooted forms/expansions — so the `expanded` Vec and the
        // `forms` slice now hold **stale** handles, even though the data survives on
        // the root stack. Re-read the live, relocated handles from the root stack for
        // the analysis passes below. Layout: `forms` at `roots_base..+n`, their
        // expansions at `roots_base+n..+2n` (pushed in pass 1, in order).
        let n = forms.len();
        let forms: Vec<Value> = (0..n).map(|j| heap.root_at(roots_base + j)).collect();
        let expanded: Vec<Value> = (0..n).map(|j| heap.root_at(roots_base + n + j)).collect();
        // Pass 2: collect every `(def name …)` in the expanded tree (top level
        // *or* nested — `defn` inside `test`/`describe`/`when`/… still defines a
        // global once it runs, so the checker honours that). `defmacro` stays a
        // special form (it doesn't expand to `def`), so we match it too.
        let mut ctx = Ctx::default();
        ctx.set_strict(strict);
        // Whole-file mode: enable operand / value-slot unbound checking (every
        // top-level def is accumulated below, and the project image is loaded, so an
        // unresolved operand is genuinely unbound — not the ambiguous free variable a
        // bare fragment might carry).
        ctx.enable_operand_checks();
        // The set of namespace prefixes the loaded image knows — every `mod/` for which
        // some `mod/<name>` global exists (the requires above are already evaluated). A
        // qualified reference whose module isn't here can't be proven unbound (it may be
        // defined dynamically or in an unloaded file), so the unbound check stays silent
        // on it; a typo in a *known* module is still flagged. See `Ctx::known_ns`.
        // Cached + shared (`Heap::known_ns_prefixes`): rebuilding this by scanning all globals
        // per file was the residual O(files²) after the header-eval redesign — an O(1) `Arc`
        // clone on all but the first file of a whole-project check.
        ctx.set_known_ns_arc(heap.known_ns_prefixes());
        // KI-17: arm the unrequired-module lint. `required_mods` is what THIS file
        // pulls in (its `:use`/`require` + own ns); `raw_qualified` restricts the lint
        // to references the user literally wrote. In a whole-project check every module
        // is bound image-wide, so a user-written `mod/name` whose `mod` is absent here
        // resolves only by load-order luck — the walk flags it. (In single-file mode an
        // un-required module isn't bound at all, so the lint is naturally inert there.)
        let mut required = collect_required_modules(heap, &forms, file_ns);
        required.extend(extra_required.iter().cloned());
        // Package-rooted namespaces (ADR-070): a `(:use b)`/`require` target roots to
        // `pkg/b` at load, and the resolved `pkg/b/name` references this lint checks are
        // rooted too — so the reachable set must carry each entry's ROOTED form as well,
        // or an intra-package reference is falsely flagged "unrequired". `root_module_name`
        // is identity for an external / std / already-rooted name, so the unrooted entries
        // stay valid too (both forms are kept). Inert outside a package context.
        let rooted: Vec<String> = required
            .iter()
            .map(|m| value::symbol_name(heap.root_module_name(value::intern(m))))
            .collect();
        required.extend(rooted);
        ctx.set_required_mods(required);
        ctx.set_raw_qualified(collect_raw_qualified(heap, &forms));
        for &form in &expanded {
            collect_def_names(heap, form, &mut ctx);
        }
        // Pass 2.5: collect `(sig name (… -> …))` declarations — the authoritative
        // signatures the call-check consults first. Two sources, both fed through the
        // same parsers + namespace-qualification (`register_declared_sig`):
        //
        //  (a) the *un-expanded* top-level `(sig …)` forms — a hand-written declaration
        //      (the `sig` macro's own output is dropped from the analysed tree, so the
        //      un-expanded form is where a plain top-level sig is legible); and
        //  (b) every `(%register-sig 'name 'type)` in the *expanded* tree — which is
        //      what BOTH a `(sig …)` and a *macro-emitted* sig lower to. `defrecord`
        //      expands to `(sig …)` forms nested in its `(do …)`, invisible to (a) (which
        //      only sees the `(defrecord …)` head); (b) recovers them. Idempotent overlap
        //      with (a) for a hand-written sig — the second register is a no-op.
        //
        // The declared name is qualified to the file's namespace exactly as the resolve
        // pass qualifies a def head and the call site — `(sig g …)` inside `(defmodule
        // ns …)` keys the sig under `ns/g`, matching the `ns/g` the walk resolves the
        // call head to. Without this a module-local sig was silently dropped (keyed bare
        // `g` while the call resolved to `ns/g`), so no user `(sig …)` in a module ever
        // reached the call-argument check.
        let file_ns_name: Option<String> = file_ns.map(value::symbol_name);
        // Populate the sealed-ability-as-a-type table (ADR-181) from the expanded tree +
        // registry BEFORE parsing sigs, so a `(sig f (Shape -> …))` / `:-> Shape` referring
        // to a sealed ability (this file's or an imported one) resolves to the union of its
        // members' record shapes rather than being dropped as an unknown type name.
        annot::set_ability_types(protocol::ability_type_table(heap, &expanded));
        // Which ids are records — the tiebreak for an unqualified sealed member that also
        // spells a built-in kind (`ratio`, `map`, …). See `annot::sealed_members_ty`.
        annot::set_record_ids(protocol::record_id_names(heap, &expanded));
        // ADR-190: build the ability facts + the sealed-op occurrence-typing domains HERE —
        // before any pass runs `sig_of` (Gap A, Pass 2.8, the body walk) — so an imported
        // function's inferred sig is never cached *without* the sealed-op demand. Keyed by op
        // name off `AbilityInfo`, which sees this file's abilities AND imported ones (via the
        // heap registries), so the demand fires for a same- or other-file sealed op alike.
        let ability_info = std::sync::Arc::new(protocol::build_ability_info(heap, &expanded));
        annot::set_sealed_op_domains(protocol::build_sealed_op_domains(&ability_info));
        for &form in &forms {
            register_declared_sig(heap, &mut ctx, file_ns_name.as_deref(), form);
        }
        // Reconstruct a `(sig name type)` form from each `%register-sig` in the expanded
        // tree (building forms needs `&mut heap`, so collect first, register after — GC
        // is blocked for the whole check, so the handles stay live).
        let mut macro_sig_forms: Vec<Value> = Vec::new();
        for &form in &expanded {
            collect_register_sig_forms(heap, form, &mut macro_sig_forms);
        }
        for &form in &macro_sig_forms {
            register_declared_sig(heap, &mut ctx, file_ns_name.as_deref(), form);
        }
        // Pass 2.7: infer a current value type for an *undeclared* global defined
        // exactly once by `(def g <non-fn-expr>)` (Gap A — docs/type-gating.md). The
        // RHS's `expr_ty` becomes `g`'s current-image type, consulted by `expr_ty` /
        // `gradual_of` as `dynamic_within` (the `∩` relation — reload-safe, warns only
        // on provable disjointness). Skipped for: a declared global (authoritative,
        // handled by `add_inferred_value_ty`); a global defined more than once
        // (ambiguous type → stays `dynamic()`); a macro; and a function/native value
        // (its arrow is inferred separately, and gating a bare function name used as a
        // value is a different concern).
        {
            let mut def_count: HashMap<Symbol, usize> = HashMap::new();
            for &form in &expanded {
                count_defs(heap, form, &mut def_count);
            }
            for &form in &expanded {
                let Some((name, rhs)) = def_name_and_value(heap, form) else {
                    continue;
                };
                // Skip a global defined more than once (ambiguous), a macro, and a
                // **dynamic variable** (`defdyn`): a dynvar's `def` sets only the
                // default, but `binding` rebinds it to any type in a dynamic extent, so
                // its value type isn't fixed — it must stay `dynamic()`.
                // Also skip an **earmuffed** `*name*` global: it is dynamic by convention
                // (reassigned over its lifetime — the lazy-init `(when (nil? *g*) (def *g*
                // …))` pattern is common), so pinning it to its load-time default value would
                // false-positive once it is reassigned — and, now that Pass 2.8 infers a
                // function's return from its body, that narrow value would propagate into the
                // return of any function that reads the global (e.g. `telemetry--metrics`).
                // `global_value_ty` already skips earmuffed globals; this makes the inferred
                // (Gap A) path — consulted first — agree.
                if def_count.get(&name) != Some(&1)
                    || ctx.is_file_macro(name)
                    || value::is_dynamic(name)
                    || infer::is_earmuffed(name)
                {
                    continue;
                }
                if let Some(ty) = infer::expr_ty(heap, rhs, &ctx) {
                    if !ty.contains_tag(value::Tag::Fn) && !ty.contains_tag(value::Tag::Native) {
                        ctx.add_inferred_value_ty(name, ty);
                    }
                }
            }
        }
        // Pass 2.8: **same-file function inference.** The file being checked isn't
        // loaded, so `sigs::sig_of`'s loaded-closure inference can't see its own `(defn …)`s
        // — a same-file caller got no checking (only cross-module callers of *loaded*
        // functions did). Infer each single-def, unshadowed, un-declared function's return
        // from its FORM (`sigs::infer_return_from_form`) and record it in `ctx`, so a
        // same-file call flows the result exactly as a loaded-function call already does.
        //
        // A bounded fixpoint resolves callees leaf-up: `infer_return_from_form` returns
        // `None` (defers, records nothing) until every function its return depends on is
        // already recorded, so a function is only ever stored with its callees' FINAL sigs —
        // no stale/narrow intermediate leaks (sound at any cap; a cross-function cycle or a
        // chain deeper than the cap simply stays deferred). The stored sig now also carries
        // the function's inferred **parameter demands** (ADR-190), so a same-file caller's
        // arguments are checked — sound because those demands under-constrain (a superset of
        // the true type, so a flagged arg genuinely errors). Runs after Gap A so a
        // `(def g <expr>)` value type its body reads is already in scope.
        {
            let mut def_count: HashMap<Symbol, usize> = HashMap::new();
            for &form in &expanded {
                count_defs(heap, form, &mut def_count);
            }
            let candidates: Vec<(Symbol, Value)> = top_level_defs(heap, &expanded)
                .into_iter()
                .filter_map(|form| {
                    let (name, rhs) = def_name_and_value(heap, form)?;
                    (def_count.get(&name) == Some(&1)
                        && !ctx.is_file_macro(name)
                        && !value::is_dynamic(name)
                        && !value::is_gensym(name)
                        && ctx.declared_sig(name).is_none())
                    .then_some((name, rhs))
                })
                .collect();
            // Iterate to a fixed point; break as soon as a pass records nothing new. The cap
            // bounds a pathological deep chain (the tail just stays deferred — sound).
            for _ in 0..16 {
                let mut changed = false;
                for &(name, rhs) in &candidates {
                    if let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), &ctx) {
                        if ctx.inferred_fn_sig(name).map(|s| s.ret) != Some(ret.clone()) {
                            // ADR-190: carry the inferred parameter demands too, so a same-file
                            // caller's arguments are checked (not just the return). Sound:
                            // `infer_params_from_form` under-constrains, so a flagged arg is one
                            // that genuinely errors. Params are body-derived (independent of this
                            // fixpoint, which only resolves returns), so recomputing is stable.
                            let params =
                                sigs::infer_params_from_form(heap, rhs, &ctx).unwrap_or_default();
                            ctx.add_inferred_fn_sig(name, crate::types::Sig::new(params, ret));
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            // A **multi-arm** candidate has no single signature — Pass 2.8's fixpoint
            // above records nothing for it — so record each arm's own instead, and the
            // call check rules a call out only when every arity-relevant arm rejects it.
            for &(name, rhs) in &candidates {
                if let Some(arms) = sigs::infer_overload_from_form(heap, rhs, &ctx) {
                    ctx.add_inferred_overload(name, arms);
                }
            }
            // …and from the **un-expanded** `defn`, which is the only place a
            // `:when`-guarded multi-clause definition still has clauses: it lowers to a
            // single variadic `fn` over `match*` (ADR-226), so the expanded tree shows a
            // rest-list being destructured and nothing about what each clause takes.
            for &form in &forms {
                let Some((name, clauses)) = defn_clauses(heap, form) else {
                    continue;
                };
                if let Some(arms) = sigs::infer_overload_from_clauses(heap, &clauses, &ctx) {
                    let qn = qualify_decl_name(&ctx, file_ns_name.as_deref(), name);
                    ctx.add_inferred_overload(qn, arms);
                }
            }
            // A candidate whose return stayed **deferred** (e.g. it returns an ability op the
            // ability facts aren't on `ctx` for yet) still gets its inferred param demands
            // (ADR-190) with an `ANY` return — so its callers are argument-checked even without
            // a resolved return. Runs after the fixpoint, so the return dynamics are untouched.
            for &(name, rhs) in &candidates {
                if ctx.inferred_fn_sig(name).is_none() {
                    if let Some(params) = sigs::infer_params_from_form(heap, rhs, &ctx) {
                        ctx.add_inferred_fn_sig(
                            name,
                            crate::types::Sig::new(params, crate::types::Ty::ANY),
                        );
                    }
                }
            }
        }
        // Pass 2.85 (ADR-259): **the declaration must be readable, and it must match what it
        // annotates.** A `(sig …)` is trusted ahead of every other signature source, so
        // a declaration the parser silently drops is worse than none at all — the
        // annotated position widens to `any` and the author is told nothing. Three ways
        // that used to happen without a word: a misspelled type name or constructor
        // (`strng`, `(tupel int)`); a sig whose parameter count contradicts its `defn`
        // (which *suppressed* the correct arity check — the call then type-checked clean
        // and died at run time); and a sig for a name the file never defines.
        check_sig_declarations(heap, &forms, file_ns_name.as_deref(), &ctx, &mut out);
        // With the signature passes done, hand the per-function answer to a `file_signatures`
        // capture if one is armed (a no-op otherwise).
        capture_file_signatures(heap, &expanded, &ctx);
        // Pass 2.6: protocol/behaviour conformance. Model `(defprotocol …)` /
        // `(defbehaviour …)` (from the un-expanded forms + the runtime registry of
        // imported ones), then check that every `(defimpl …)` provides each declared op
        // at the right arity, and every `(:implements …)` module *defines* them (read
        // from the expanded tree, so macro-generated defns count).
        let protocols = protocol::collect(heap, &forms);
        protocol::check_impls(heap, &forms, &protocols, &mut out);
        protocol::check_behaviours(heap, &forms, &expanded, &protocols, &mut out);
        // Ability call-site checks: the syntactic pass (literals / direct ctor args) runs
        // now; the same facts go into `ctx` so `check_into`'s inference hook can also flag
        // a record-typed *variable* passed to an op with no impl.
        protocol::check_ability_calls(heap, &expanded, &ability_info, &mut out);
        protocol::check_sealed(&ability_info, &mut out);
        // Super-ability conformance (ADR-193): an id implementing an ability must also implement
        // that ability's `:requires` prerequisites.
        protocol::check_requires(&ability_info, &mut out);
        // Op names must be unique within a module: two abilities declaring the same op
        // name would clobber each other's generic function (ADR-172).
        protocol::check_op_collisions(&ability_info, &mut out);
        // Multimethod missing-method: a direct `defmulti` generic call whose full argument
        // tuple is statically known but has no exact method and no `:default` (ADR-179).
        let multi_info = std::sync::Arc::new(protocol::build_multi_info(heap, &expanded));
        protocol::check_multi_calls(heap, &expanded, &multi_info, &mut out);
        ctx.set_multi(multi_info);
        ctx.set_ability(ability_info);
        // Ability impl-return conformance: an op declaring `:-> RET` has each of its
        // impls' bodies checked against that return type (gradual, false-positive-clean).
        // Runs with `ctx` carrying the ability facts just set.
        walk::check_impl_returns(heap, &expanded, &ctx, &mut out);
        // Pass 3: check each expanded form with the accumulated file-globals, plus the
        // names *this* form guards with `(bound? 'name)` — a deliberately conditional
        // reference to an ambient another module defines, which must not read as unbound.
        for &form in &expanded {
            let mut guarded = HashSet::new();
            collect_bound_guards(heap, form, &mut guarded);
            ctx.set_bound_guarded(guarded);
            check_into(heap, form, &ctx, &mut out);
        }
        ctx.set_bound_guarded(HashSet::new()); // the exemption is per-form; don't leak it
                                               // Pass 3.5: flag non-tail self-recursion (overflow footgun — Brood loops
                                               // must be tail-recursive). Walks the same expanded tree.
        for &form in &expanded {
            recursion::check_recursion(heap, form, &mut out);
        }
        // Pass 3.6: sealed-`match` exhaustiveness (ADR-187 part 2). Reads the *un-expanded*
        // forms (a `match` survives only pre-expansion) with `ctx` carrying the file's sigs
        // and abilities, so a scrutinee typed as a sealed ability resolves to its record-id
        // set. Sound: anything it can't prove total defers to silence.
        // (No backfill here: this pass reads the UN-expanded forms and positions each warning
        // at its `match`, which the reader positioned. A backfill from "some top-level form"
        // would put a wrong location on a warning, which is worse than none.)
        exhaustive::check_matches(heap, &forms, &ctx, &mut out);
        // Pass 3.7: guard purity (advisory) — flag an effectful primitive in a `:when`
        // guard. Also reads the *un-expanded* forms (a `:when` guard lowers to a plain
        // `if`-test on expansion). A guard runs on rejected clauses and, in a `receive`,
        // re-runs per mailbox scan, so an effect there fires on paths never selected.
        guard_effects::check_guards(heap, &forms, &mut out);
        // (The macro binding-capture lint was retired when automatic binding hygiene
        // shipped — ADR-066 amendment: a template's `let`/`fn` binders are alpha-renamed
        // to fresh gensyms by the expander, so a plain literal binder can no longer
        // capture spliced caller code and the lint would only false-positive.)
        // Pass 4.5: unused `(:use …)` imports — a `:use` clause that contributes no
        // symbol ever referenced in the file's expanded forms. Read the `:use` module
        // names from the *unexpanded* header (the clause is gone after expansion), then
        // group the imported qualified names by source module and scan the expanded tree
        // for references. Warn only when the module contributed ≥1 public name and none
        // appear.
        {
            // A file with an unresolved BARE reference has, by construction, an import
            // table that doesn't cover everything it names — so "module M contributed
            // nothing you reference" is not provable: the name we failed to resolve may
            // be M's. Emitting both is self-contradictory advice, and the dangerous half
            // is this one ("delete the import your code needs"). So the lint stands down
            // whenever the unbound diagnostic fired on a bare name. It stays fully live
            // for a file that resolves cleanly, which is the case it exists for.
            let unresolved_bare = out.iter().any(|(_, m)| {
                m.strip_prefix("unbound symbol: ")
                    .is_some_and(|rest| !rest.split(' ').next().unwrap_or("").contains('/'))
            });
            let use_modules = extract_use_module_names(heap, &forms);
            if !use_modules.is_empty() && !unresolved_bare {
                let all_refs = collect_all_syms(heap, &expanded);
                let imported = heap.imported_pairs();
                // Group each module's contributed names by source module. We record BOTH
                // the qualified name AND the local (unqualified) alias the file actually
                // calls — a `(:use …)` import is normally referenced *unqualified*
                // (`*green*`, not `theme/*green*`), so matching only the qualified form
                // (the old bug) reported every unqualified-used import as unused.
                let mut module_names: std::collections::HashMap<String, Vec<Symbol>> =
                    std::collections::HashMap::new();
                for (local, qual) in &imported {
                    let qname = value::symbol_name(*qual);
                    if let Some(slash) = qname.rfind('/') {
                        let e = module_names.entry(qname[..slash].to_string()).or_default();
                        e.push(*local);
                        e.push(*qual);
                    }
                }
                // Module prefixes referenced via a *qualified* `mod/name` symbol anywhere
                // in the file — so a file that reaches a module only by qualified reference
                // (including to its module-private names, which aren't imported at all) still
                // counts the `:use` as load-bearing.
                let mut qualified_mods: HashSet<String> = HashSet::new();
                for &s in &all_refs {
                    let n = value::symbol_name(s);
                    if let Some(slash) = n.rfind('/') {
                        qualified_mods.insert(n[..slash].to_string());
                    }
                }
                for mod_name in &use_modules {
                    // Only actionable when the module contributed importable names (it's in
                    // the table) — a failed require / macro-only / empty module is silent.
                    if let Some(names) = module_names.get(mod_name) {
                        let used = names.iter().any(|s| all_refs.contains(s))
                            || qualified_mods.contains(mod_name);
                        if !used {
                            out.push((None, format!("unused :use import: {mod_name}")));
                        }
                    }
                }
            }
        }
        // (Unused module-private `defn`s are checked at the *whole-project* layer
        // — `std/tool/project.blsp` `project-unused-private-warnings` — not here: a
        // `--` name is a convention, not enforced privacy, so the editor legitimately
        // references it from other modules and tests by its qualified name, which a
        // single-file pass can't see. A per-file check produced false positives.)
    }));
    // Balance the GC roots pushed for pass 1 (input forms + their expansions) and
    // restore the compile-namespace state — on the clean AND the panic path.
    heap.truncate_roots(roots_base);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_ns_known_by_module(prev_by_module);
    heap.set_imports(prev_imports);
    if let Err(p) = panicked {
        let msg = if let Some(s) = p.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        out.push((
            None,
            format!("checker internal error (advisory pass aborted; please report): {msg}"),
        ));
    }
    out
}

/// A `(def NAME RHS)` form → `(NAME, RHS)`. `None` for anything else — a
/// non-`def` head, a `(defmacro …)` (which stays a special form, never expands to
/// `def`), or a malformed def. Used by Gap A's undeclared-global type inference.
/// A signature reshaped to the definition's actual parameter list, filling anything the
/// inference did not type with `any`.
///
/// This is what makes a suggestion *writable*: the LSP's declare-sig action and
/// `nest check --suggest-sigs` both hand a `(sig …)` to a human to paste, and one whose
/// arity contradicts its `defn` is not a weaker suggestion — it is a broken one that the
/// declaration checker rejects. `string/last-index-of (s needle &optional before)`
/// inferred `(-> int)`; it now reads `(any any &optional any -> int)`, which says less
/// about the parameters and the truth about the shape.
fn reconcile_arity(heap: &Heap, value_form: Value, sig: Sig) -> Sig {
    let Some(arity) = walk::fn_form_arity(heap, value_form) else {
        return sig;
    };
    let required = arity.min;
    // **Fill in, never overrule.** A multi-clause `defn` lowers to a single variadic
    // `fn` over `match*` (ADR-226), so the *form's* arity says "any number of arguments"
    // while the clause inference knows the real one — reshaping to the form would throw
    // that away and offer `(...any) -> …`. So this only acts on the case it exists for:
    // an inference that produced no parameters at all for a definition that has some.
    if sig.params.len() >= required {
        return sig;
    }
    let mut params: Vec<Ty> = (0..required)
        .map(|i| sig.params.get(i).cloned().unwrap_or(Ty::ANY))
        .collect();
    let mut optional = sig.optional.clone();
    let mut rest = sig.rest.clone();
    match arity.max {
        // Unbounded: the tail is a rest binder.
        None => rest = rest.or(Some(Ty::ANY)),
        Some(max) => {
            let extra = max.saturating_sub(required);
            while optional.len() < extra {
                optional.push(Ty::ANY);
            }
        }
    }
    params.truncate(required);
    Sig {
        params,
        optional,
        rest,
        ret: sig.ret,
    }
}

/// The top-level definition forms of a file — each form as written, with the `(do …)`
/// that `defn-`/`def-` expand to opened up one level.
///
/// `defn-`/`def-` (ADR-146) expand to `(do (def name …) (%mark-private 'name))`, so a
/// pass reading `expanded` directly finds no definition at all for a **module-private**
/// function — and most definitions in a real module are private (40 of
/// `std/json.blsp`'s 42). A pass keyed on [`def_name_and_value`] therefore has to look
/// inside, or it silently declines to infer anything about them and their call sites go
/// unchecked.
///
/// **Only** that shape, identified by the `%mark-private` call it carries. Other macros
/// emit a top-level `do` too, and opening those was tried and reverted — twice over:
///
/// - the linear-map rewrite wraps a fold's result in `(do (def linmap-out__N …) …)`, and
///   typing that temporary made the checker flag a branch of the rewrite's own wrapper
///   that cannot run with that value — a warning naming a symbol the author never wrote
///   (caught live by `tests/linmap_soundness_test.blsp`); and
/// - `defability`/`defimpl` define their ops inside one, and an *inferred* signature for
///   an op displaces the `:-> T` return the ability declares, so the return stopped
///   flowing to call sites (`ability_op_return_type_flows_to_call_site`).
///
/// Both consumers additionally skip [`value::is_gensym`] names on their own account: a
/// gensym is invisible to the author, so a diagnostic naming one is never actionable.
fn top_level_defs(heap: &Heap, expanded: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(expanded.len());
    for &form in expanded {
        match walk::do_body(heap, form).filter(|body| marks_private(heap, body)) {
            Some(body) => out.extend(body),
            None => out.push(form),
        }
    }
    out
}

/// Does this `do` body carry a `(%mark-private …)` call — the fingerprint of a
/// `defn-`/`def-` expansion?
fn marks_private(heap: &Heap, body: &[Value]) -> bool {
    body.iter().any(|&form| {
        list_items(heap, form)
            .and_then(|items| items.first().copied())
            .is_some_and(
                |head| matches!(head, Value::Sym(s) if value::symbol_is(s, "%mark-private")),
            )
    })
}

fn def_name_and_value(heap: &Heap, form: Value) -> Option<(Symbol, Value)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, kw::DEF) {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    Some((name, items[2]))
}

/// Every name `form` guards with an explicit `(bound? 'name)` test, recursively.
///
/// A `(bound? 'x)` test says, in the source itself, "this name may not exist in this
/// image" — the prelude's ability dispatch reads `*project-name*` / `*ns-package*` that
/// way, since those ambients are `defdyn`'d by `std/tool/project.blsp` and simply absent
/// under a bare `brood script.blsp`. Flagging the guarded reference as unbound reports a
/// *correct* program, which is exactly what the advisory contract forbids
/// (ADR-123..126) — so the guarded name is exempt for the top-level form that guards it.
///
/// Scoped to one top-level form (usually one `defn`) rather than the whole file so a
/// `bound?` probe in one function can't silence a genuine typo in another.
fn collect_bound_guards(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(head)) = items.first() {
            if value::symbol_is(head, "bound?") {
                // `(bound? 'name)` reads as `(bound? (quote name))` in the expanded tree.
                if let Some(quoted) = items.get(1).copied().and_then(|q| list_items(heap, q)) {
                    if matches!(quoted.first(), Some(&Value::Sym(q)) if value::symbol_is(q, kw::QUOTE))
                    {
                        if let Some(&Value::Sym(name)) = quoted.get(1) {
                            out.insert(name);
                        }
                    }
                }
            }
        }
        for &item in items.iter() {
            collect_bound_guards(heap, item, out);
        }
    })
}

/// Count every `(def NAME …)` for each name across `form`, **including nested** defs (a
/// reassignment inside a function body, a `do`, a `when`, …) — not just top-level. A global
/// def'd more than once anywhere is *reassigned*, so its type isn't its first value; the
/// value-type inference (Gap A) and the same-file return inference (Pass 2.8) both use this
/// to leave such a global `dynamic()` rather than pinning it to a stale initial value (the
/// lazy-init `(when (nil? *g*) (def *g* (table)))` pattern — else a function returning `*g*`
/// infers `nil` and false-flags its callers). Skips quoted subtrees.
fn count_defs(heap: &Heap, form: Value, counts: &mut HashMap<Symbol, usize>) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
            return;
        }
        if value::symbol_is(head, kw::DEF) {
            if let Some(&Value::Sym(name)) = items.get(1) {
                *counts.entry(name).or_insert(0) += 1;
            }
        }
        for &item in items.get(1..).unwrap_or(&[]) {
            count_defs(heap, item, counts);
        }
    })
}

/// [`check_file`] plus the Phase-2 incremental-cache **dep-keys** for the file: the
/// serializable set of global observations the check made (see [`deps`]). Runs the
/// same check under a dependency recorder. Returns `(warnings, dep_keys)`; feed
/// `dep_keys` to [`deps_fingerprint`] to get the stamp a later run compares against.
pub fn check_file_with_deps(
    heap: &mut Heap,
    forms: &[Value],
) -> (Vec<(Option<Pos>, String)>, Value) {
    check_file_with_deps_ext(heap, forms, &[])
}

/// [`check_file_with_deps`] with the KI-17 reachability set (see [`check_file_ext`]).
pub fn check_file_with_deps_ext(
    heap: &mut Heap,
    forms: &[Value],
    extra_required: &[String],
) -> (Vec<(Option<Pos>, String)>, Value) {
    deps::begin_record(heap);
    let warnings = check_file_ext(heap, forms, extra_required);
    let dep_keys = deps::take_dep_keys(heap);
    (warnings, dep_keys)
}

/// The current-image fingerprint for a file's `dep_keys` (from
/// [`check_file_with_deps`]). Equal across two runs iff every observed global fact
/// is unchanged — the soundness core of the incremental check cache (ADR-119).
pub fn deps_fingerprint(heap: &Heap, dep_keys: Value) -> String {
    deps::fingerprint(heap, dep_keys)
}

#[cfg(test)]
mod tests;

/// **Soundness oracles.** An advisory, never-gating checker can't have classic
/// type soundness, but it has two facets that *are* directly testable — and both
/// guard the under-approximation bug class the B1 `negate` fix was about:
///
/// - **(I) Result soundness** — the static type [`expr_ty`] assigns is a
///   *superset* of what the expression evaluates to. The checker may **widen**,
///   never under-approximate; a too-small result type can make
///   [`Ty::is_disjoint`] fire on correct code. Tested by evaluating each closed
///   expression and asserting the runtime value is a *member* of its static type.
/// - **(II) No false positives** — a program that evaluates *without a runtime
///   type error* must draw no type-disjointness (`expects … got`) or
///   callback-arity warning. Any such warning on runnable, correct code is a
///   false positive, which the checker promises never to emit (contract #5).
///   This is the facet that exercises the narrowing / `negate` else-branch path.
///
/// **Add a case here whenever you add a result-typing or narrowing rule** to
/// `seq_aware_call_ty` / `expr_ty` / the guard pipeline.
#[cfg(test)]
mod soundness_oracle;
