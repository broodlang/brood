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
//! - [`hygiene`] — the macro-hygiene lint: a `defmacro` template whose literal
//!   binder can capture spliced caller code.
//! - [`protocol`] — protocol / behaviour conformance: `defprotocol` /
//!   `defbehaviour` / `defimpl` / `(:implements …)` checked for missing or
//!   wrong-arity ops.
//! - [`recursion`] — the non-tail self-recursion lint (deep non-tail recursion
//!   overflows the green-process stack).
//!
//! ## Where signatures come from (Step 3)
//!
//! Three sources, simplest-first — *no inference engine* (`docs/types.md`):
//!
//! 1. **Primitives** — every [`NativeFn`](crate::core::value::NativeFn) carries
//!    a `Sig` ([contract point #6, enforced](../docs/types.md#compatibility-contract))
//!    so the checker just reads it from the global env (see
//!    [`sigs::primitive_sig`]). There is no parallel table to maintain.
//! 2. **Curated stdlib** — a small hand-vetted table for the variadic /
//!    `reduce`-based / higher-order Brood closures the checker can't infer but
//!    that matter (`+ - * / < <= > >= mod map filter reduce`; see
//!    [`sigs::curated_sig`]). Each is a Brood `defn`, but its sig is pinned by hand.
//! 3. **Basic inference** for a closure whose body is **one straight-line
//!    expression** (a single direct call to a known sig; no `if`/`cond`/`let`/
//!    `match`/recursion). Each closure parameter inherits the type the callee
//!    expects at the position(s) where the parameter is passed; the closure's
//!    return is the callee's. Sound because a straight-line use is
//!    unconditional — no control-flow analysis (see [`sigs::sig_of`]).
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
//! Not yet (later increments): inference through `cond`/`match`, structured /
//! `and`/`or`-chained guards, recursion, higher-order. The checker runs
//! automatically as the pre-flight in `brood <file>` / `nest test` / `nest run`
//! / `nest check`; the in-process entry points are [`check_file`] (whole file)
//! and the `(check 'form)` builtin (a fragment).
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

mod annot;
mod ctx;
pub(super) mod deps;
mod guards;
mod hygiene;
mod infer;
mod protocol;
mod recursion;
mod sigs;
mod walk;

use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self as value, Symbol, Value};
use crate::error::Pos;

use ctx::Ctx;
use walk::{check_into, collect_all_syms, collect_def_names, list_items};

/// True when `form` is a top-level `(require …)` call — the one form the
/// checker pre-evaluates so a module's macros (e.g. `defprocess` from
/// `std/proc/gen.blsp`) are resolvable for the rest of the file.
fn is_require_form(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, "require");
        }
    }
    false
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
            // Each (:use mod …) clause starts with the :use keyword.
            if let Some(items) = list_items(heap, clause) {
                if let Some(Value::Keyword(kw_sym)) = items.first() {
                    if value::symbol_is(*kw_sym, "use") {
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
        Use(Symbol, Option<Vec<Symbol>>), // (module, Some(only-subset) | None = all public)
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
                // `:only`/`:refer` marker → refer just the listed names; else all public.
                let subset = match (citems.get(2), citems.get(3)) {
                    (Some(Value::Keyword(m)), Some(sub))
                        if value::symbol_is(*m, "only") || value::symbol_is(*m, "refer") =>
                    {
                        // `:only [a b]` uses a VECTOR literal (also accepts a list), so read it
                        // with `seq_items` (vector + list) — NOT `list_items` (cons-only), which
                        // would miss a vector and silently fall through to "import all". Read-only,
                        // no alloc → no GC, so holding `items`/`citems` across it is safe.
                        heap.seq_items(*sub).ok().map(|ns| {
                            ns.iter()
                                .filter_map(|n| match n {
                                    Value::Sym(s) => Some(*s),
                                    _ => None,
                                })
                                .collect()
                        })
                    }
                    _ => None,
                };
                clauses.push(Clause::Use(mod_sym, subset));
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
    for clause in clauses {
        match clause {
            Clause::Use(mod_sym, subset) => {
                let mod_name = value::symbol_name(mod_sym);
                let prefix = format!("{}/", mod_name);
                // Load the module if absent (no `mod/*` globals) — the standalone path; in a
                // whole-project check it's already loaded, so this is skipped.
                if deps::obs_module_exports(heap, &prefix).is_empty() {
                    let quoted = heap.list(vec![
                        Value::Sym(value::intern("quote")),
                        Value::Sym(mod_sym),
                    ]);
                    let form = heap.list(vec![Value::Sym(value::intern("require")), quoted]);
                    let root = heap.global();
                    let _ = crate::eval::eval(heap, form, root); // advisory: swallow load errors
                }
                match subset {
                    Some(names) => {
                        for bare in names {
                            let qual = value::intern(&format!(
                                "{}/{}",
                                mod_name,
                                value::symbol_name(bare)
                            ));
                            heap.add_import(bare, qual);
                        }
                    }
                    None => {
                        for (bare, qual) in deps::obs_module_exports(heap, &prefix) {
                            heap.add_import(bare, qual);
                        }
                    }
                }
            }
            Clause::UseInternals(mod_sym) => {
                let key = crate::eval::macros::internals_grant_key(&value::symbol_name(mod_sym));
                heap.add_import(key, mod_sym);
            }
            Clause::Alias(short, mod_sym) => {
                let key = value::intern(&format!("{}/", value::symbol_name(short)));
                heap.add_import(key, mod_sym);
            }
        }
    }
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
    // (`defprocess`, `cast`, `!`, etc. from `std/proc/gen.blsp`) would expand as
    // an un-known head and trip the unbound-symbol diagnostic. `require` is
    // idempotent (it checks `*features*`), so a later real run re-evaluating
    // the same form is a no-op. Failures are swallowed: the checker is
    // advisory and shouldn't gate on a missing module.
    let root = heap.global();
    // Namespace-aware checking (ADR-065): if the file declares `(ns foo)`, set the
    // compile namespace + forward-ref pre-scan so pass 1's resolve qualifies both
    // definition heads and references to `foo/…` — otherwise every qualified
    // reference would look unbound. Restored before returning.
    let file_ns = crate::eval::macros::file_ns(heap, forms);
    let prev_ns = heap.set_compile_ns(file_ns);
    let prev_known = if file_ns.is_some() {
        heap.set_ns_known_names(crate::eval::macros::scan_def_names(heap, forms))
    } else {
        heap.set_ns_known_names(std::collections::HashSet::new())
    };
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
                Err(err) => {
                    out.push((err.pos, format!("does not compile: {}", err.message)));
                    f
                }
            };
            // Root the just-built expansion *before* possibly triggering a
            // collect via `eval`; otherwise this LOCAL handle dies between
            // here and the next iteration's macroexpand.
            heap.push_root(exp);
            expanded.push(exp);
            // Make the file's imports + required modules resolvable for the rest of the walk.
            // For a `(defmodule … (:use …))` header, populate the import table DIRECTLY from its
            // clauses (`setup_check_imports`) instead of evaling the expanded header — the eval
            // re-macroexpands + re-compiles it and runs per-file O(globals) `provide`/`require`/
            // `%refer` scans, which made a whole-project check O(files²). A standalone
            // `(require …)` form (not a header) is still evaluated so its macros/globals resolve.
            if is_ns_header(heap, f) {
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
        annot::set_ability_types(protocol::sealed_member_ids(heap, &expanded));
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
                if let Some((name, _)) = def_name_and_value(heap, form) {
                    *def_count.entry(name).or_insert(0) += 1;
                }
            }
            for &form in &expanded {
                let Some((name, rhs)) = def_name_and_value(heap, form) else {
                    continue;
                };
                // Skip a global defined more than once (ambiguous), a macro, and a
                // **dynamic variable** (`defdyn`): a dynvar's `def` sets only the
                // default, but `binding` rebinds it to any type in a dynamic extent, so
                // its value type isn't fixed — it must stay `dynamic()`.
                if def_count.get(&name) != Some(&1)
                    || ctx.is_file_macro(name)
                    || value::is_dynamic(name)
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
        let ability_info = std::sync::Arc::new(protocol::build_ability_info(heap, &expanded));
        protocol::check_ability_calls(heap, &expanded, &ability_info, &mut out);
        protocol::check_sealed(&ability_info, &mut out);
        // Op names must be unique within a module: two abilities declaring the same op
        // name would clobber each other's generic function (ADR-172).
        protocol::check_op_collisions(&ability_info, &mut out);
        // Multimethod missing-method: a direct `defmulti` generic call whose full argument
        // tuple is statically known but has no exact method and no `:default` (ADR-179).
        let multi_info = protocol::build_multi_info(heap, &expanded);
        protocol::check_multi_calls(heap, &expanded, &multi_info, &mut out);
        ctx.set_ability(ability_info);
        // Ability impl-return conformance: an op declaring `:-> RET` has each of its
        // impls' bodies checked against that return type (gradual, false-positive-clean).
        // Runs with `ctx` carrying the ability facts just set.
        walk::check_impl_returns(heap, &expanded, &ctx, &mut out);
        // Pass 3: check each expanded form with the accumulated file-globals.
        for &form in &expanded {
            check_into(heap, form, &ctx, &mut out);
        }
        // Pass 3.5: flag non-tail self-recursion (overflow footgun — Brood loops
        // must be tail-recursive). Walks the same expanded tree.
        for &form in &expanded {
            recursion::check_recursion(heap, form, &mut out);
        }
        // Pass 4: macro-hygiene lint over the *un-expanded* forms — `defmacro`
        // templates and their `~unquote` structure only survive pre-expansion
        // (`macroexpand_all` leaves quasiquote opaque, and the template is gone once
        // a macro is applied). Reads only.
        for &form in &forms {
            hygiene::check_macro_hygiene(heap, form, &mut out);
        }
        // Pass 4.5: unused `(:use …)` imports — a `:use` clause that contributes no
        // symbol ever referenced in the file's expanded forms. Read the `:use` module
        // names from the *unexpanded* header (the clause is gone after expansion), then
        // group the imported qualified names by source module and scan the expanded tree
        // for references. Warn only when the module contributed ≥1 public name and none
        // appear.
        {
            let use_modules = extract_use_module_names(heap, &forms);
            if !use_modules.is_empty() {
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
                // (including to its private `--` names, which aren't imported at all) still
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
        // — `std/tool/project.blsp` `project--unused-private-warnings` — not here: a
        // `--` name is a convention, not enforced privacy, so the editor legitimately
        // references it from other modules and tests by its qualified name, which a
        // single-file pass can't see. A per-file check produced false positives.)
    }));
    // Balance the GC roots pushed for pass 1 (input forms + their expansions) and
    // restore the compile-namespace state — on the clean AND the panic path.
    heap.truncate_roots(roots_base);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
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

/// [`check_file`] plus the Phase-2 incremental-cache **dep-keys** for the file: the
/// serializable set of global observations the check made (see [`deps`]). Runs the
/// same check under a dependency recorder. Returns `(warnings, dep_keys)`; feed
/// `dep_keys` to [`deps_fingerprint`] to get the stamp a later run compares against.
pub fn check_file_with_deps(
    heap: &mut Heap,
    forms: &[Value],
) -> (Vec<(Option<Pos>, String)>, Value) {
    deps::begin_record(heap);
    let warnings = check_file(heap, forms);
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
