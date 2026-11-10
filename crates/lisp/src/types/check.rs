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
//! - [`protocol`] — ability / behaviour conformance: `defability` /
//!   `defbehaviour` / `impl` / `(:implements …)` checked for missing or
//!   wrong-arity ops.
//! - [`recursion`] — the non-tail self-recursion lint (deep non-tail recursion
//!   overflows the green-process stack).
//! - [`module_loading`] / [`imports`] — what the check loads, and the file's header
//!   imports, requires and world.
//! - [`declarations`] — the file's `(sig …)` declarations and the type vocabulary they
//!   may name; [`top_level`] — readers of its top-level definition forms.
//! - [`same_file_inference`] — the file's own functions inferred from their forms, and
//!   the caller-derived parameter fixpoint.
//! - [`unused_binders`] / [`file_lints`] — the surface binder lints and the whole-file
//!   lint passes; [`signature_capture`] — the effective signatures behind inlay hints.
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
//! 2. **Declared and curated** — a std `(sig …)` (read from the loaded module or the
//!    image's signature index), and a small hand-vetted table for the closures nothing
//!    else types (`map`, `reduce`, `io/puts`; see [`sigs::curated_sig`]). The full
//!    precedence is stated once, on [`sigs::sig_of`].
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
//! multi-arity/variadic closure *returns*, a file's own functions (`check_file`'s same-file inference
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
mod cross_module_def;
mod ctx;
pub(super) mod deps;
mod discarded_catch;
mod exhaustive;
mod guard_effects;
mod guards;
mod infer;
mod known_ops;
mod properties;
mod protocol;
mod recursion;
mod sigs;
pub(crate) use properties::effect_of;
pub(crate) use sigs::cover_name_of;

/// Whether `name` has a curated signature (`sigs::curated_sig`) — read ahead of the stdlib
/// image's footer and of inference, so its module is never loaded on its account. For
/// `tests/curated_names_load_nothing.rs`.
pub fn is_curated(name: &str) -> bool {
    sigs::has_curated_sig(value::intern(name))
}

/// Every name with a curated signature — for `tests/curated_sigs_exist.rs`, which holds each
/// to existing (a curated name is KNOWN to the checker, so a stale one silences its unbound
/// lint).
#[doc(hidden)]
pub fn curated_sig_names() -> Vec<String> {
    sigs::curated_names()
        .into_iter()
        .map(|sym| value::symbol_name_ref(sym).to_string())
        .collect()
}
mod std_index;
pub(crate) use std_index::{module_signature_index, std_signature_index};
mod walk;

mod declarations;
mod file_lints;
mod imports;
mod module_loading;
mod same_file_inference;
mod signature_capture;
mod top_level;
mod unused_binders;
pub(crate) use imports::ImportScope;
pub use imports::module_direct_requires;
pub use module_loading::materialise_referenced_modules;
pub use signature_capture::{FnSignature, file_signatures};

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self as value, Arity, Symbol, Value};
use crate::error::Pos;
use crate::types::{Sig, Ty};

use crate::core::registries as reg;
use ctx::Ctx;
use declarations::*;
use file_lints::*;
use imports::*;
use module_loading::*;
use same_file_inference::*;
use signature_capture::*;
use top_level::*;
use unused_binders::*;
use walk::{check_into, collect_all_syms, collect_def_names, list_items};

/// The **type signature** of the callable `sym` resolves to — declared, curated, or
/// inferred (all of `sigs::sig_of`'s sources) — rendered as its arrow string, e.g.
/// `(int -> int)` or `(fn seqable -> seqable)`. `None` for a non-callable, an unknown
/// name, or one whose signature can't be pinned. The tooling-facing view of the
/// inferencer (LSP hover, docs); reads only, never gates. The per-file inference memo is
/// cleared first so a re-edited/reloaded function re-infers rather than showing a stale sig.
pub fn signature_string(heap: &Heap, sym: Symbol) -> Option<String> {
    sigs::clear_sig_memo();
    // A declared sig may name a `deftype` alias (ADR-327): install the loaded aliases, or
    // the declaration fails to parse and the answer silently falls back to the inferred
    // one — `(model any -> model)` read as `(any, any) -> map`.
    annot::set_type_aliases(
        protocol::type_alias_table(heap, &[], None),
        None,
        ImportScope::default(),
    );
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
    annot::set_type_aliases(HashMap::new(), None, ImportScope::default());
    // …and the inference memos, whose entries were computed against that file's Ctx
    // tables (`fn_form`, `clause_arms`) — `signature_string` already does this.
    sigs::clear_sig_memo();
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
/// so the threading macro (`->`), pattern syntax (`match`), test framework
/// wrappers (`test`/`describe`/…), and any user macro that rearranges code
/// are checked against their *expanded* shape — not the surface syntax that
/// would otherwise mistake `(map inc)` inside `(-> xs (map inc))` for a
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
/// Arm re-typings the call-site specializer spent on the LAST file checked on this
/// thread — the checker's own work meter, for a test that bounds it (KI-138). Reset at the
/// start of every [`check_file`], so read it straight after one.
pub fn specialization_fuel_spent() -> u32 {
    sigs::fuel_spent()
}

/// Expression visits the inferencer made on the LAST file checked on this thread — the
/// finer meter beside [`specialization_fuel_spent`]: that one counts body walks, this one
/// counts what each walk cost (KI-139). Reset at the start of every [`check_file`].
pub fn expr_ty_visits_spent() -> u64 {
    infer::expr_ty_visits()
}

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

/// [`check_file`] with the file's *transitive* require-closure (module names), computed
/// by the whole-project driver (`std/tool/project.blsp`, which alone sees every file's
/// header). Unioned with the file's own direct requires (which `check_file_ext` derives
/// from `forms`), it seeds the file's world — what its registry reads may see (B8).
/// Pass `&[]` for the single-file / LSP path.
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
    check_forms(heap, forms, extra_required, strict, false)
}

/// [`check_file`] for forms that are about to be EVALUATED HERE — a REPL line, an
/// editor's `C-x C-e` — rather than loaded as a file: when the forms declare no
/// `(defmodule …)` of their own, they are checked under this process's current compile
/// context (the namespace `%in-ns` last opened and its `(:use …)` imports), which is
/// exactly how `eval` will resolve them. Without it a form pulled out of a module's
/// buffer reads every bare import as unbound and the module's own `deftype`s as
/// unknown, so the checker's verdict on it describes a different program than the one
/// that runs.
pub fn check_forms_here(heap: &mut Heap, forms: &[Value]) -> Vec<(Option<Pos>, String)> {
    check_forms(heap, forms, &[], super::strict_checking(), true)
}

/// A top-level `(def name …)` (what `defn`/`def-`/… expand to) of a RESERVED name — one
/// the language ships, ADR-166 — always fails when it runs (`eval`'s `def` arm refuses
/// it before evaluating the value), so the checker says so up front instead of passing a
/// file that cannot load. The same predicate the runtime asks, `Heap::is_reserved_global`
/// on the name as `def` roots it, so the two cannot disagree about the list.
///
/// Only outside a module: inside `(defmodule m …)` the expanded head is already `m/name`,
/// which is reserved only when `m` is an embedded std module — and loading that module is
/// exactly the one context the runtime exempts, so checking std's own sources must not
/// warn on every definition in them. For the same reason a form read from one of the
/// prelude's own files (`boot::PRELUDE_PATHS`) is the definition, not a redefinition —
/// the file-checking primitives set the heap's current file for the walk.
fn reserved_def_warning(heap: &Heap, expanded: Value) -> Option<String> {
    if heap.compile_ns().is_some() {
        return None;
    }
    if heap
        .current_file()
        .is_some_and(crate::boot::is_prelude_source)
    {
        return None;
    }
    let items = list_items(heap, expanded)?;
    let (Some(Value::Sym(head)), Some(Value::Sym(name))) = (items.first(), items.get(1)) else {
        return None;
    };
    if value::symbol_name_ref(*head) != kw::DEF {
        return None;
    }
    let name = *name;
    let name = heap.root_qualified_ref(name).unwrap_or(name);
    heap.is_reserved_global(name).then(|| {
        format!(
            "`{}` is a reserved name — it ships with Brood and cannot be redefined, so this \
             definition fails when it runs; pick another name, or define it inside a \
             `(defmodule …)`",
            value::symbol_name(name)
        )
    })
}

fn check_forms(
    heap: &mut Heap,
    forms: &[Value],
    extra_required: &[String],
    strict: bool,
    inherit_context: bool,
) -> Vec<(Option<Pos>, String)> {
    let mut out = Vec::new();
    // Block the copying GC for the whole check: LOCAL handles are held in Rust `Vec`s
    // across the `eval` of `(require …)` forms, and a collection would relocate them.
    // Inner evals then run at depth ≥ 2, so the outermost-eval safepoint never fires
    // mid-check (ADR-054); the checker's allocations are reclaimed at the next safepoint.
    let _gc_block = crate::process::GcBlockGuard::enter();
    // Every inferred load is EAGER for the duration (ADR-335): the unbound verdict for a
    // qualified typo (`json/prase`) depends on `json` being loaded, so the checker loads
    // what the file names where a program would load it on first use.
    let _eager = crate::eval::derive::EagerLoadScope::enter();
    // Per-file state: one file's inferred sigs, abilities and aliases must not leak into
    // the next, nor (in the long-lived LSP) survive a source edit (KI-13).
    sigs::clear_sig_memo();
    annot::clear_ability_types();
    annot::set_type_aliases(HashMap::new(), None, ImportScope::default());
    // The namespace the file declares, ROOTED as `%in-ns` roots it (ADR-065/070), so
    // the resolve qualifies definition heads and references exactly as the runtime does —
    // otherwise the `--` privacy rule reads a module's own helper as a foreign access.
    let declared_ns = crate::eval::macros::file_ns(heap, forms).map(|ns| heap.root_module_name(ns));
    // Forms checked "here" (`check_forms_here`) that open no module of their own are
    // resolved in the namespace already open, as `eval` would resolve them.
    let inherit = inherit_context && declared_ns.is_none();
    let file_ns = if inherit {
        heap.compile_ns()
    } else {
        declared_ns
    };
    // `*load-path*` is a global the expansion pass may change (`is_load_path_form`). Save
    // and restore it so one file's path setup cannot change how the NEXT file of a
    // project check resolves its modules (a relative entry names a different directory
    // from each file). Safe to hold across the check — GC is blocked.
    let load_path_sym = crate::core::value::intern(reg::LOAD_PATH);
    let saved_load_path = heap.env_get(heap.global(), load_path_sym);
    let prev_ns = heap.set_compile_ns(file_ns);
    // Region model (ADR-223): a file may declare more than one `(defmodule …)`. Install the
    // per-module forward-ref pre-scan and start on the FIRST module's region; the
    // expansion pass switches at each later `defmodule` (the checker doesn't `eval`
    // `%in-ns`, so it mirrors that switch itself).
    let regions = if declared_ns.is_some() {
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
    // (its `(require …)`/`%refer` is evaluated like any other header form). Checked
    // "here", the process's current imports are the header.
    let inherited_imports: crate::core::heap::ImportTable = if inherit {
        heap.imports_snapshot().into_iter().collect()
    } else {
        crate::core::heap::ImportTable::default()
    };
    let prev_imports = heap.set_imports(inherited_imports);
    // The input forms and their expansions are rooted from here (a module load can
    // collect); `truncate_roots` below balances them even when a panic unwinds.
    let roots_base = heap.roots_len();
    // Panic containment: the checker is advisory and runs inside long-lived hosts
    // (brood-lsp, `nest check`, the REPL's background check) — an internal panic must not
    // tear the host down, nor leave the compile-ns / known-names / imports / GC roots
    // captured above poisoned for the next check. The restores below run on BOTH paths,
    // and a panic degrades to one "checker internal error" diagnostic.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check_file_passes(
            heap,
            forms,
            roots_base,
            extra_required,
            strict,
            file_ns,
            &mut out,
        )
    }));
    // Balance the GC roots pushed while expanding (input forms + their expansions) and
    // restore the compile-namespace state — on the clean AND the panic path.
    heap.truncate_roots(roots_base);
    if let Some(saved) = saved_load_path {
        let global = heap.global();
        heap.env_define(global, load_path_sym, saved);
    }
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_ns_known_by_module(prev_by_module);
    heap.set_imports(prev_imports);
    if let Err(payload) = panicked {
        let message = if let Some(text) = payload.downcast_ref::<&str>() {
            (*text).to_string()
        } else if let Some(text) = payload.downcast_ref::<String>() {
            text.clone()
        } else {
            "non-string panic payload".to_string()
        };
        out.push((
            None,
            format!("checker internal error (advisory pass aborted; please report): {message}"),
        ));
    }
    // One finding per site: a macro that copies a sub-form into two branches — `match`
    // falls through to a clause from both its tag test and its shape test — walks the same
    // positioned source twice. First occurrence wins, so the order is unchanged.
    let mut seen = HashSet::new();
    out.retain(|(pos, message)| seen.insert((pos.map(|p| (p.line, p.col)), message.clone())));
    out
}

/// Where a file's check spent its time, printed under `BROOD_DERIVE_DBG`.
#[derive(Default)]
struct DeriveTiming {
    /// The whole joint fixpoint: derivation walks plus return re-inference.
    joint: std::time::Duration,
    /// The derivation walks alone.
    deriving: std::time::Duration,
    rounds: u32,
    site_walks: u32,
    site_reuses: u32,
}

fn derive_debug() -> bool {
    crate::diagnostics::debug_flags::derive_debug()
}

/// Every pass of a file check, in the order they run. Each pass reads what the ones
/// before it recorded on `ctx`, so the order is load-bearing.
fn check_file_passes(
    heap: &mut Heap,
    forms: &[Value],
    roots_base: usize,
    extra_required: &[String],
    strict: bool,
    file_ns: Option<Symbol>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let file_started = std::time::Instant::now();
    let (forms, expanded, import_scope, absent_ns) = expand_and_load(heap, forms, roots_base, out);
    let (mut ctx, visible) =
        collect_file_scope(heap, &forms, &expanded, extra_required, strict, file_ns);
    ctx.set_absent_ns(absent_ns);
    let file_ns_name: Option<String> = file_ns.map(value::symbol_name);
    let multi_info =
        std::sync::Arc::new(protocol::build_multi_info(heap, &expanded, Some(&visible)));
    install_type_vocabulary(
        heap,
        &expanded,
        file_ns_name.as_deref(),
        import_scope,
        &multi_info,
        out,
    );
    let ability_info = register_declarations(
        heap,
        &forms,
        &expanded,
        &mut ctx,
        file_ns_name.as_deref(),
        &visible,
    );
    infer_global_value_types(heap, &expanded, &mut ctx);
    let candidates = infer_same_file_functions(
        heap,
        &forms,
        &expanded,
        &mut ctx,
        file_ns_name.as_deref(),
        out,
    );
    let mut timing = DeriveTiming::default();
    let joint_started = std::time::Instant::now();
    derive_caller_params(heap, &expanded, &candidates, &mut ctx, out, &mut timing);
    timing.joint = joint_started.elapsed();
    // A `(sig …)` is trusted ahead of every other signature source, so one the parser
    // drops, or that contradicts its `defn`, is worse than none (ADR-259).
    check_sig_declarations(heap, &forms, file_ns_name.as_deref(), &ctx, out);
    lint_duplicate_defs(heap, &forms, out);
    // With the signatures settled, hand them to a `file_signatures` capture if armed.
    capture_file_signatures(heap, &expanded, &ctx);
    ctx.set_multi(multi_info.clone());
    check_conformance(
        heap,
        &forms,
        &expanded,
        &ability_info,
        &multi_info,
        &ctx,
        out,
    );
    protocol::lint_single_dispatch_multis(heap, &expanded, &multi_info, out);
    // The body walk, with the names each form guards with `(bound? 'name)` — a
    // deliberately conditional reference to an ambient another module defines, which
    // must not read as unbound. The exemption is per form.
    for &form in &expanded {
        let mut guarded = HashSet::new();
        collect_bound_guards(heap, form, &mut guarded);
        ctx.set_bound_guarded(guarded);
        check_into(heap, form, &ctx, out);
    }
    ctx.set_bound_guarded(HashSet::new());
    run_shape_lints(heap, &forms, &expanded, &ctx, out);
    lint_unused_imports(heap, &forms, &expanded, out);
    report_exhausted_budgets(out);
    if derive_debug() {
        eprintln!(
            "[derive] {}: joint fixpoint {} ms ({} ms deriving over {} rounds, {} form walks, {} reused) of {} ms",
            file_ns_name.as_deref().unwrap_or("<no module>"),
            timing.joint.as_millis(),
            timing.deriving.as_millis(),
            timing.rounds,
            timing.site_walks,
            timing.site_reuses,
            file_started.elapsed().as_millis(),
        );
    }
}

/// Macroexpand and namespace-resolve every form, so the analysed tree matches what `eval`
/// will see, and make the file's imports and required modules resolvable for the rest of
/// the check. Returns the forms and their expansions re-read from the root stack (a
/// module load can collect at any depth and relocate them), the header's import scope,
/// and the prefixes of the qualified references no module could satisfy
/// ([`absent_module_prefixes`]).
///
/// A `(defmodule … (:use …))` header populates the import table directly from its
/// clauses (`setup_check_imports`) rather than evaluating the expanded header, which
/// re-runs per-file O(globals) scans and made a whole-project check O(files²). A
/// standalone `(require …)` or `*load-path*` form is evaluated, so the macros and globals
/// it brings in resolve. A compile error is a real diagnostic (the file would fail to
/// load); the un-expanded form still feeds the walk so the other lints run.
fn expand_and_load(
    heap: &mut Heap,
    forms: &[Value],
    roots_base: usize,
    out: &mut Vec<(Option<Pos>, String)>,
) -> (Vec<Value>, Vec<Value>, ImportScope, HashSet<String>) {
    let root = heap.global();
    for &f in forms {
        heap.push_root(f);
    }
    let n = forms.len();
    let mut import_scope = ImportScope::default();
    for j in 0..n {
        // Re-read the form from the root stack, NOT the `forms` slice: an earlier
        // iteration's module load can collect at any depth (ADR-061) and relocate it.
        let form = heap.root_at(roots_base + j);
        // The checker reads the author's code: no optimiser source rewrites here.
        let _plain = crate::eval::macros::NoSourceRewrites::enter();
        let expansion = match crate::eval::macros::compile(heap, form, root) {
            Ok(expansion) => expansion,
            Err(err) => {
                // A macro's own `(error …)` during expansion carries no position; the
                // form it was expanding does. Report there rather than nowhere.
                out.push((
                    err.pos.or_else(|| heap.form_pos_only(forms[j])),
                    format!("does not compile: {}", err.message),
                ));
                // `compile` infers `(require …)` from qualified references, and that load
                // can collect, so `form` above is stale here.
                heap.root_at(roots_base + j)
            }
        };
        // Root the expansion before anything below can collect.
        heap.push_root(expansion);
        let form = heap.root_at(roots_base + j);
        if let Some(message) = reserved_def_warning(heap, expansion) {
            out.push((heap.form_pos_only(form), message));
        }
        if is_ns_header(heap, form) {
            // A later `(defmodule …)` opens a new region (ADR-223): switch the compile-ns
            // and forward-ref set to it, as `%in-ns` does at run time.
            if let Some(module) = crate::eval::macros::defmodule_form_name(heap, form) {
                let rooted = heap.root_module_name(module);
                heap.set_compile_ns(Some(rooted));
                heap.activate_ns_region(module);
            }
            import_scope.extend(setup_check_imports(heap, form));
        } else if is_require_form(heap, expansion) || is_load_path_form(heap, expansion) {
            let _ = crate::eval::eval(heap, expansion, root);
        }
    }
    // What the modules loaded so far refer to, transitively — so the inference passes read
    // a loaded body all the way down to the leaf that declares its type.
    materialise_referenced_modules(heap);
    // Layout on the root stack: `forms` at `roots_base..+n`, expansions at `+n..+2n`.
    let read_rooted = |heap: &Heap| -> (Vec<Value>, Vec<Value>) {
        (
            (0..n).map(|j| heap.root_at(roots_base + j)).collect(),
            (0..n).map(|j| heap.root_at(roots_base + n + j)).collect(),
        )
    };
    // Which modules the file names that nothing could load. Asked HERE, before the final
    // re-read, because asking evaluates the loader's probe, which may collect.
    let (forms, expanded) = read_rooted(heap);
    let absent = absent_module_prefixes(heap, &forms, &expanded);
    let (forms, expanded) = read_rooted(heap);
    (forms, expanded, import_scope, absent)
}

/// The file's scope on a fresh `Ctx`: every name it defines (top level or nested — a
/// `defn` inside `describe` still defines a global once it runs), the module prefixes the
/// image knows, the modules it requires, and the qualified names it writes. Returns the
/// `Ctx` and the file's WORLD — what its registry reads may see.
fn collect_file_scope(
    heap: &mut Heap,
    forms: &[Value],
    expanded: &[Value],
    extra_required: &[String],
    strict: bool,
    file_ns: Option<Symbol>,
) -> (Ctx, HashSet<String>) {
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
    // The file's WORLD (B8): what its registry reads may see. It starts from what THIS
    // file pulls in (its `:use`/`require` + own ns, plus the driver's require-closure).
    let mut visible = collect_required_modules(heap, forms, file_ns);
    visible.extend(extra_required.iter().cloned());
    // Package-rooted namespaces (ADR-070): a `(:use b)`/`require` target roots to
    // `pkg/b` at load, and the resolved `pkg/b/name` references are rooted too — so the
    // set must carry each entry's ROOTED form as well. `root_module_name` is identity for
    // an external / std / already-rooted name, so the unrooted entries stay valid too
    // (both forms are kept). Inert outside a package context.
    let rooted: Vec<String> = visible
        .iter()
        .map(|m| value::symbol_name(heap.root_module_name(value::intern(m))))
        .collect();
    visible.extend(rooted);
    // A std module reached only by a qualified `tempo/new!` is loaded on first use
    // (ADR-335) and needs no `:use`, so every user-written qualified prefix counts too —
    // and the set is closed through the standard library: the driver closes over PROJECT
    // files, so a std module is a leaf there, but what it requires is in the file's world
    // as well (`tempo` reaches `datetime`), and a value the file can construct must never
    // be read as invisible.
    visible.extend(
        collect_raw_qualified(heap, forms)
            .iter()
            .filter_map(|q| q.rfind('/').map(|slash| q[..slash].to_string())),
    );
    close_through_std(heap, &mut visible);
    for &form in expanded {
        collect_def_names(heap, form, &mut ctx);
    }
    (ctx, visible)
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

/// [`check_file_with_deps`] with the file's require-closure (see [`check_file_ext`]).
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
