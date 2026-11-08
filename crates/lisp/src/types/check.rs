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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self as value, Arity, Symbol, Value};
use crate::error::Pos;
use crate::types::{Sig, Ty};

use crate::core::registries as reg;
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

/// A top-level form that changes **where** a module is found, and so has to be
/// evaluated in pass 1 for the same reason `(require …)` is: the whole file is
/// checked before any of it runs, so a path the program sets up for itself does
/// not exist yet when the forms that depend on it are walked.
///
/// Without this, a file that puts its own directory on the path and then uses a
/// MACRO from it had that macro read as an unknown head — and an unknown head's
/// arguments are walked as if they were evaluated code, so every operand of the
/// macro was reported as an unbound symbol. Found on wos, an OS written in
/// Brood, whose `(reflect/add-load-path "compiler")` precedes an assembler macro:
/// nine false warnings per build on a file that assembles, boots and prints.
///
/// Evaluating it is what makes the checker MORE complete here, not less: with
/// the module resolved, `resolves_to_macro` recognises the head, the existing
/// macro rule leaves its arguments alone, and every *other* form in the file —
/// including the module's own definitions — is checked normally. Silencing the
/// walk instead would have traded one false positive for a blind spot.
///
/// The cost, stated plainly: `*load-path*` is runtime-wide, so it is WIDER for the
/// duration of the check. `check_forms` restores it, but a concurrent process
/// resolving a module inside that window sees the wider path. That is the same
/// property evaluating a `(require …)` already has — it loads the module globally
/// — so this extends an existing trade rather than opening a new one.
fn is_load_path_form(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        let (head, _) = heap.pair(p);
        if let Value::Sym(s) = head {
            return crate::core::value::symbol_is(s, "reflect/add-load-path")
                || crate::core::value::symbol_is(s, "reflect/set-load-path");
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
    let features = heap.env_get(heap.global(), value::intern(reg::FEATURES));
    match features {
        Some(Value::Map(mid)) => heap.map_get(mid, key).is_some(),
        _ => false,
    }
}

/// Load `mod_sym` unless it is already loaded (the FEATURE registry, exactly what the
/// runtime `require-one` consults — see `setup_check_imports` for why "some `mod/*` global
/// exists" was the wrong test). Advisory: a load error is swallowed, the checker never gates
/// on a missing module. `require-one` is idempotent, so a spurious call costs a map lookup.
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

/// Materialise, transitively, every module that the LOADED modules' bodies refer to by a
/// qualified name — `text` for a loaded `editor/buffer`, whose `buffer-current-line` calls
/// `text/char->line`.
///
/// A program loads such a module on the first call that reaches the name (ADR-335's lazy
/// policy: the global-lookup miss path), and the checker never calls. The stdlib image
/// records only the edges the policy followed at build time — header `(:use …)`s and macro
/// heads — so materialising a module from it brings in none of the modules its plain function
/// references name. The checker's inference then reads a loaded body up to the first
/// unmaterialised qualified name and stops: `buffer-current-line` inferred `-> any`, though
/// `text/char->line` DECLARES `(rope int -> int)`, and every derivation above it — the whole
/// point of declaring the leaf — was lost, in `nest check` and the LSP alike.
///
/// Fixpoint: scan each loaded module's bindings once (a per-thread memo, since a project
/// check runs this per file), collect the module prefixes its closures' bodies mention, load
/// the ones not yet loaded, and go again while something new loaded. A prefix is tried once
/// per thread; a name that is not a module (a load error) is swallowed like every other
/// advisory load. Runs under the check's `EagerLoadScope`, and before the passes that infer.
///
/// `pub` for `tests/curated_names_load_nothing.rs`, which observes this scan's decision in
/// a process of its own (whether a std module is loaded is process-wide state once an image
/// is installed, so the in-crate suite cannot hold the precondition).
pub fn materialise_referenced_modules(heap: &mut Heap) {
    thread_local! {
        static SCANNED: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
        static TRIED: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
    }
    loop {
        let unscanned: Vec<Symbol> = loaded_feature_modules(heap)
            .into_iter()
            .filter(|m| SCANNED.with(|s| s.borrow_mut().insert(*m)))
            .collect();
        if unscanned.is_empty() {
            return;
        }
        // One pass over the globals for all of this round's modules.
        let prefixes: Vec<String> = unscanned
            .iter()
            .map(|m| format!("{}/", value::symbol_name(*m)))
            .collect();
        let mut wanted: HashSet<Symbol> = HashSet::new();
        let mut names: HashSet<Symbol> = HashSet::new();
        let global_env = heap.global();
        let members =
            heap.global_symbols_where(|name| prefixes.iter().any(|p| name.starts_with(p.as_str())));
        for g in members {
            let Some(Value::Fn(cid)) = heap.env_get(global_env, g) else {
                continue;
            };
            let bodies: Vec<Value> = heap
                .closure(cid)
                .arms
                .iter()
                .flat_map(|arm| arm.body.iter().copied())
                .collect();
            for body in bodies {
                collect_qualified_names(heap, body, &mut names);
            }
        }
        // ADR-370: a name whose DECLARED signature the image carries asks for no load — the
        // checker reads its type from the footer. A name without one (an undeclared
        // function, a value, a module no image describes) still brings its module in, so
        // the body walk that infers it has the leaf.
        let trace = crate::diagnostics::debug_flags::image_trace();
        let mut names: Vec<Symbol> = names.into_iter().collect();
        names.sort_by_key(|s| value::symbol_name(*s));
        for name in names {
            // A name already BOUND — a native registered under a module's prefix
            // (`string/split`, `file/stat`), a prelude binding, a loaded module's def — is
            // read from its binding; loading `std/string.blsp` for `string/split` gives the
            // checker nothing it does not have. The eager drain's own rule (KI-150). Both
            // skips sit under the one lever, so `BROOD_NO_IMAGE_SIGS=1` is exactly the
            // pre-ADR-370 whole-module scan and the differential covers both.
            //
            // A CURATED name (`sigs::curated_sig`) is read from the table ahead of the
            // footer and ahead of inference (`sig_of`'s order), so its verdict never depends
            // on the load either: `seq/filter` is curated and named by 38 std modules'
            // bodies, `io/puts` by 26, and each check materialised `seq` and `io` for a type
            // it already had.
            if crate::eval::derive::image_sigs_enabled()
                && (heap.env_get(global_env, name).is_some()
                    || crate::eval::derive::image_sig_text(heap, name).is_some()
                    || sigs::has_curated_sig(name))
            {
                continue;
            }
            let text = value::symbol_name_ref(name);
            if let Some(slash) = text.rfind('/')
                && slash > 0
            {
                let module = value::intern(&text[..slash]);
                // The insert is the LOAD SET — it must not sit behind the trace flag.
                // ADR-370's first shape wrote `trace && wanted.insert(module)`, and the
                // short-circuit made the whole transitive scan a no-op in every process
                // that did not set `BROOD_IMAGE_TRACE` (KI-171): the one test of it ran its
                // child WITH the trace. `transitive_scan_loads_without_the_trace` pins it.
                wanted.insert(module);
                // The question KI-150 took days to answer by experiment — WHICH
                // references made the check load a module — printed per name in sorted
                // order: every one is a name to declare a signature for, and the load
                // stays until the LAST of them has one.
                if trace
                    && heap.env_get(global_env, module).is_none()
                    && !feature_loaded(heap, &text[..slash])
                {
                    eprintln!("[image] check materialises {} for {text}", &text[..slash]);
                }
            }
        }
        let mut loaded_any = false;
        // Load in name order, not hash order: a load has side effects (its own warnings,
        // registry writes), and the same file must do them the same way every run (B8).
        let mut wanted: Vec<Symbol> = wanted.into_iter().collect();
        wanted.sort_by_key(|m| value::symbol_name(*m));
        for module in wanted {
            if !TRIED.with(|t| t.borrow_mut().insert(module)) {
                continue;
            }
            if feature_loaded(heap, &value::symbol_name(module)) {
                continue;
            }
            ensure_loaded(heap, module);
            loaded_any |= feature_loaded(heap, &value::symbol_name(module));
        }
        if !loaded_any {
            return;
        }
    }
}

/// The names in the `*features*` registry — every module loaded in this process.
fn loaded_feature_modules(heap: &Heap) -> Vec<Symbol> {
    let Some(Value::Map(mid)) = heap.env_get(heap.global(), value::intern(reg::FEATURES)) else {
        return Vec::new();
    };
    heap.map_entries(mid)
        .into_iter()
        .filter_map(|(k, _)| match k {
            Value::Str(id) => Some(value::intern(heap.string(id))),
            _ => None,
        })
        .collect()
}

/// Every qualified symbol anywhere in `form` — `math/quot`, `editor/buffer/point` — descending
/// through lists, vectors and maps. The root escape `/name` and a bare name contribute nothing.
/// Over-approximate on purpose (quoted data counts): the only cost of a spurious name is one
/// swallowed load of its module.
fn collect_qualified_names(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) => {
                let name = value::symbol_name_ref(s);
                if name.rfind('/').is_some_and(|slash| slash > 0) {
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
}

/// Qualify a `(sig …)` declaration's target name to the file's namespace, the way
/// the resolve pass qualifies a def head — but only with the same *positive
/// evidence* the resolver requires: the file actually defines `ns/name` (recorded
/// as a file-global in pass 2). A name the file doesn't define (an imported/prelude
/// name), an already-`ns/`-qualified name, or an ambient `*earmuffed*` name is left
/// as written, so it still matches the bare/qualified head the call resolves to.
fn qualify_decl_name(heap: &Heap, ctx: &Ctx, file_ns: Option<&str>, name: Symbol) -> Symbol {
    match file_ns {
        Some(ns) => {
            let qualified = crate::eval::macros::qualify_name(heap, ns, name);
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
    // The property keywords — `:pure`, `:total` — of any sig shape (ADR-351).
    if let Some((name, _, props)) = annot::sig_decl_head(heap, form)
        && !props.is_empty()
    {
        let qn = qualify_decl_name(heap, ctx, file_ns, name);
        ctx.add_declared_props(qn, &props);
    }
    if let Some((name, sig)) = annot::parse_sig_decl(heap, form) {
        let qn = qualify_decl_name(heap, ctx, file_ns, name);
        ctx.add_declared_sig(qn, sig);
        // `(T… -> _)` — the result is left to inference (ADR-420).
        if let Some((_, Some(type_form), _)) = annot::sig_decl_head(heap, form)
            && annot::arrow_ret_is_open(heap, type_form)
        {
            ctx.mark_open_ret(qn);
        }
    }
    if let Some((name, sv)) = annot::parse_sig_decl_with_vars(heap, form) {
        let qn = qualify_decl_name(heap, ctx, file_ns, name);
        ctx.add_declared_sig_with_vars(qn, sv);
    }
    // An overloaded sig — `(and (int -> int) (bool -> bool))` — has no single `Sig`,
    // so `parse_sig_decl` above yields nothing for it; record it separately (ADR-116).
    if let Some((name, sigs)) = annot::parse_sig_decl_overload(heap, form) {
        let qn = qualify_decl_name(heap, ctx, file_ns, name);
        ctx.add_declared_overload(qn, sigs);
    }
    // Non-arrow `(sig x T)` value-type declarations — consumed by the gradual-
    // assignment check on `(def x …)` (the first `GradualTy` consumer).
    if let Some((name, ty)) = annot::parse_value_sig_decl(heap, form) {
        let qn = qualify_decl_name(heap, ctx, file_ns, name);
        ctx.add_declared_value_ty(qn, ty);
    }
}

/// A surface `(defn name clause…)` / `(defn- name clause…)` whose tail is entirely
/// clauses — arity clauses `((a b) …)` or pattern clauses `(((x y & acc) "+") …)` —
/// returning the name and those clauses. `None` for a single-clause definition (which
/// the ordinary parameter inference already reads) or any other form. A clause is a list
/// whose head is a list (`is_clause`'s shape; a vector head is Clojure's spelling and is
/// rejected by the expander), which is why `(defn f ([a b]) body)` — one destructuring
/// clause followed by a body form — is not two clauses.
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
    let is_clause = |f: Value| -> bool {
        list_items(heap, f)
            .is_some_and(|items| matches!(items.first(), Some(Value::Pair(_)) | Some(Value::Nil)))
    };
    if rest.len() < 2 || !rest.iter().all(|&f| is_clause(f)) {
        return None;
    }
    Some((name, rest.to_vec()))
}

/// **Unused pattern-binder lint**, run on the UN-EXPANDED tree.
///
/// A pattern clause's binder that its own body never reads is dead, and in a fold
/// callback it is worse than dead: `(fn ((acc "+") "+") ((acc num) (string/->number num)))`
/// ignores the accumulator in both arms, so the `reduce` never folds and the result is
/// whatever the last element produced. Nothing caught this. The unused-binding lint only
/// ever looked at `let`, and by the time the checker's main walk runs, a pattern clause
/// has been lowered to `match*` — where its binders become SYNTHETIC lets, which that lint
/// deliberately exempts (removing that exemption fires 69 times across std/ + tests/, so
/// it is load-bearing). Hence a surface pass: here the clause is still as written.
///
/// Per CLAUSE, because each clause has its own binder list, so `_`-prefixing one that is
/// deliberately ignored costs nothing — the same escape the `let` lint uses. A whole-fn
/// rule would be unsound: one arm may legitimately ignore what another arm reads.
fn lint_unused_pattern_binders(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    lint_binders_walk(heap, form, true, out);
}

/// `lint_this` says whether *this* form, if it is a `fn`, is anonymous — it is cleared
/// only for the `fn` that is a definition's value. Children always reset it to true.
///
/// The distinction has to be made STRUCTURALLY, not by which tree we happen to be handed:
/// `nest check FILE` runs this over un-expanded forms (where a `defn` is still a `defn`
/// and never looked like a `fn`), while `check-string-structured` — the editor/LSP path —
/// runs it over expanded ones (where `defn` has already become `(def name (fn …))`). The
/// first version relied on that accident and so warned in the editor but not on the
/// command line for the same source, which is worse than either answer alone.
fn lint_binders_walk(
    heap: &Heap,
    root: Value,
    root_lint: bool,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    // An explicit work stack, not recursion: this walks arbitrary user source, and the
    // checker is expected to survive deeply nested forms (`checker_survives_a_deep_let_body`
    // overflows a debug-build stack on the recursive version).
    let mut work = vec![(root, root_lint)];
    while let Some((v, lint_this)) = work.pop() {
        match v {
            Value::Vector(id) => {
                work.extend(heap.vector(id).iter().map(|&e| (e, true)));
            }
            // A map or set literal evaluates its keys and values, so a `fn` written there
            // (`{:on-complete (fn (mm text) mm)}`, the editor's callback specs) is as
            // anonymous as one in a vector and is linted the same.
            Value::Map(id) => {
                for &(key, entry) in heap.map_entries(id).iter() {
                    work.push((key, true));
                    work.push((entry, true));
                }
            }
            Value::Set(id) => work.extend(heap.map_entries(id).iter().map(|&(key, _)| (key, true))),
            Value::Pair(_) => {
                let Some(items) = list_items(heap, v) else {
                    continue;
                };
                // `Value` has no `PartialEq`; the definition's value is positionally items[2].
                let mut definition_value: Option<usize> = None;
                if let Some(&Value::Sym(head)) = items.first() {
                    // Quoted/quasiquoted subtrees are data, and `comment` never runs.
                    if value::symbol_is(head, kw::QUOTE)
                        || value::symbol_is(head, kw::QUASIQUOTE)
                        || value::symbol_is(head, kw::COMMENT)
                    {
                        continue;
                    }
                    if lint_this && walk::is_fn_head(head) {
                        lint_fn_pattern_clauses(heap, &items, out);
                    }
                    lint_binding_form(heap, head, v, &items, out);
                    // `(def name (fn …))` — `defn`'s expansion. That `fn`'s parameters are
                    // the definition's PUBLISHED arglist (introspection, LSP hover, `doc`),
                    // where an unused one is frequently the contract: an ability impl, a
                    // callback of fixed shape, a dispatch arm. Only genuinely anonymous
                    // `fn`s are linted.
                    if value::symbol_is(head, kw::DEF) && items.len() >= 3 {
                        definition_value = Some(2);
                    }
                }
                for (i, it) in items.into_iter().enumerate() {
                    work.push((it, definition_value != Some(i)));
                }
            }
            _ => {}
        }
    }
}

/// The per-clause half of [`lint_unused_pattern_binders`], for an anonymous `fn` in either
/// spelling. `(fn (acc x) acc)` and `(fn ((acc x) acc))` are the same function, so they get
/// the same answer: the single-arm form is one clause whose head is the parameter list.
fn lint_fn_pattern_clauses(heap: &Heap, items: &[Value], out: &mut Vec<(Option<Pos>, String)>) {
    let forms = match items.get(1..) {
        Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
        Some(rest) => rest,
        None => return,
    };
    if forms.is_empty() {
        return;
    }
    if !forms
        .iter()
        .all(|&c| crate::eval::macros::is_clause(heap, c))
    {
        // `(fn (params…) body…)`: the parameter list is the one clause head, and a head of
        // only symbols is an arity clause, so its binders are parameters.
        if let Some((&params @ (Value::Pair(_) | Value::Nil), body)) = forms.split_first() {
            let noun = match list_items(heap, params) {
                Some(names) if names.iter().all(|p| matches!(p, Value::Sym(_))) => {
                    "unused parameter"
                }
                _ => "unused pattern binding",
            };
            lint_clause_binders(heap, params, params, body, noun, out);
        }
        return;
    }
    let heads: Vec<Value> = forms
        .iter()
        .filter_map(|&c| list_items(heap, c)?.first().copied())
        .collect();
    if heads.len() != forms.len() {
        return;
    }
    // `:when` INSIDE the head list. It is clause syntax and belongs after the list —
    // `((acc num) :when (string? num) body…)` — so a head list containing it silently
    // becomes two extra parameters, the arity stops matching, and every call dies with a
    // `{:kind :match :context :fn …}` error that names the clauses rather than the mistake. There is
    // nothing else it can plausibly be: a head list holds patterns, and a pattern matching
    // the literal keyword `:when` in a parameter position is not a thing anyone writes.
    for (&clause, &head) in forms.iter().zip(&heads) {
        let Some(ps) = list_items(heap, head) else {
            continue;
        };
        if ps
            .iter()
            .any(|p| matches!(p, Value::Keyword(k) if value::symbol_is(*k, "when")))
            && let Some(pos) = heap.form_pos_only(clause)
        {
            out.push((
                Some(pos),
                concat!(
                    "`:when` belongs after the head list, not inside it — ",
                    "write `((a b) :when (guard a) body…)`. Inside, it is two more ",
                    "parameters, so the clause stops matching and every call fails ",
                    "with a match error",
                )
                .to_string(),
            ));
        }
    }
    // A clause whose head holds a non-symbol is a PATTERN clause (the whole `fn` is then
    // lowered to `match*`); one of only symbols is an arity clause (a native multi-arity
    // arm). Both bind per-clause names, so both are linted — the wording differs so the
    // reader knows which construct is meant. It is decided once for the whole `fn`, NOT
    // per clause, because the LOWERING is per fn: one pattern head anywhere sends the
    // entire dispatch through `match*`, so every clause's binders are then pattern
    // binders, including a head that happens to be all symbols. Naming that one a
    // "parameter" would describe a construct the program does not have.
    let any_pattern = heads.iter().any(|&h| {
        list_items(heap, h).is_some_and(|ps| ps.iter().any(|p| !matches!(p, Value::Sym(_))))
    });
    let noun = if any_pattern {
        "unused pattern binding"
    } else {
        "unused parameter"
    };
    for &clause in forms {
        let Some(citems) = list_items(heap, clause) else {
            continue;
        };
        let (Some(&plist), body) = (citems.first(), &citems[1..]) else {
            continue;
        };
        lint_clause_binders(heap, clause, plist, body, noun, out);
    }
}

/// Warn on each binder of the head `plist` that `body` never reads, at `located`'s
/// position (the clause, or the parameter list of a single-arm `fn`).
fn lint_clause_binders(
    heap: &Heap,
    located: Value,
    plist: Value,
    body: &[Value],
    noun: &str,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if body.is_empty() || heap.is_synthetic(located) {
        return;
    }
    let Some(pos) = heap.form_pos_only(located) else {
        return;
    };
    let mut binders = Vec::new();
    collect_clause_binders(heap, plist, &mut binders);
    // An `&optional` default is an expression evaluated with the earlier parameters in
    // scope — `(fn (a &optional (b (fn () a))) …)` reads `a` there — so it counts as a use.
    let defaults = optional_defaults(heap, plist);
    for b in binders {
        let name = value::symbol_name(b);
        // The `_`-prefix ignore convention, and real gensym temporaries — via the
        // interner's own predicate, not a `contains("__")` guess, which also swallowed
        // a legitimate `my__thing`.
        if name.starts_with('_') || walk::is_gensym_sym(b) {
            continue;
        }
        // The same shadowing exemption the sibling unused-`let` lint applies: you do
        // not accidentally name a binder after a builtin, so a shadow left unused is a
        // deliberate scope-isolation test, and `_`-prefixing cannot express it (that
        // changes the name being shadowed).
        if sigs::is_globally_bound(heap, b) || sigs::has_curated_sig(b) {
            continue;
        }
        if !body
            .iter()
            .chain(&defaults)
            .any(|&f| walk::sym_appears_in(heap, f, b))
        {
            out.push((Some(pos), format!("{noun}: {name}")));
        }
    }
}

/// The default expressions of a head list's `&optional` slots, `(pattern default)`.
fn optional_defaults(heap: &Heap, plist: Value) -> Vec<Value> {
    let mut defaults = Vec::new();
    let mut optional = false;
    for item in list_items(heap, plist).unwrap_or_default() {
        match item {
            Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => optional = true,
            Value::Sym(s) if value::symbol_is(s, kw::AMP) => optional = false,
            _ if optional => {
                if let Some(&[_, default]) = list_items(heap, item).as_deref() {
                    defaults.push(default);
                }
            }
            _ => {}
        }
    }
    defaults
}

/// Binder symbols of a `fn` parameter list (a clause head).
///
/// A typed parameter (ADR-420) binds its name, not its spelling: `position:int` binds
/// `position`, `name: T` binds `name` and `T` is a type, and everything after `->` is the
/// result type. Read as written, every typed parameter was a binder nothing could read.
/// After `&optional`, a `(pattern default)` slot binds its pattern's names; the default is
/// an expression (ADR-619). Each parameter is itself a pattern ([`collect_pattern_binders`]).
fn collect_clause_binders(heap: &Heap, plist: Value, out: &mut Vec<Symbol>) {
    let Some(items) = heap.list_or_vector_items(plist) else {
        collect_pattern_binders(heap, plist, out);
        return;
    };
    let mut optional = false;
    let mut items = items.into_iter();
    while let Some(item) = items.next() {
        if let Value::Sym(s) = item {
            if value::symbol_is(s, crate::syntax::typed::ARROW) {
                break;
            }
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                optional = true;
                continue;
            }
            if value::symbol_is(s, kw::AMP) {
                optional = false;
                continue;
            }
            match crate::syntax::typed::split(value::symbol_name_ref(s)) {
                Some(crate::syntax::typed::TypedName::Next(name)) => {
                    out.push(value::intern(name));
                    items.next();
                }
                Some(typed) => out.push(value::intern(typed.name())),
                None => collect_pattern_binders(heap, item, out),
            }
            continue;
        }
        match list_items(heap, item).as_deref() {
            Some(&[slot, _default]) if optional => collect_pattern_binders(heap, slot, out),
            _ => collect_pattern_binders(heap, item, out),
        }
    }
}

/// The names a pattern binds, by the one pattern grammar (`docs/language.md` § Pattern
/// matching; the CST twin is `syntax::scope`'s `collect_pattern_binders`). A bare symbol
/// binds; a literal, a quoted symbol `'s` and a pin `^x` (`(%pin x)`) do not — a pin
/// COMPARES against an outer binding, so flagging it would false-positive on every pinned
/// `receive` clause (`[:down ^ref _ reason]`). `and`/`or`/`bytes` are operators over
/// sub-patterns, `(record name {…})` names a type, and a map pattern's `:or` holds
/// default values, not patterns.
fn collect_pattern_binders(heap: &Heap, pattern: Value, out: &mut Vec<Symbol>) {
    match pattern {
        Value::Sym(s) => {
            // `_` is collected like any binder; the `_`-prefix exemption silences it
            if !(value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_OPTIONAL)) {
                out.push(s);
            }
        }
        Value::Vector(id) => {
            collect_sequence_binders(heap, heap.vector(id), out);
        }
        Value::Pair(_) => {
            let items = list_items(heap, pattern).unwrap_or_default();
            let skip = match items.first() {
                Some(&Value::Sym(h)) if value::symbol_is(h, kw::QUOTE) => return,
                Some(&Value::Sym(h)) if value::symbol_is(h, "%pin") => return,
                Some(&Value::Sym(h))
                    if value::symbol_is(h, "or")
                        || value::symbol_is(h, "and")
                        || value::symbol_is(h, "bytes") =>
                {
                    1
                }
                Some(&Value::Sym(h)) if value::symbol_is(h, "record") => 2,
                _ => return collect_sequence_binders(heap, &items, out),
            };
            for &item in items.iter().skip(skip) {
                collect_pattern_binders(heap, item, out);
            }
        }
        Value::Map(id) => {
            for &(key, entry) in heap.map_entries(id).iter() {
                if !matches!(key, Value::Keyword(k) if value::symbol_is(k, "or")) {
                    collect_pattern_binders(heap, entry, out);
                }
            }
        }
        _ => {}
    }
}

/// The binders of a list or vector pattern's elements. After `&optional` a slot is
/// `(pattern default)` (or a bare name), and the default is an expression, not a pattern:
/// `(_a &optional (b (do (io/write "d") 0)))` binds `b` and nothing named `do`.
fn collect_sequence_binders(heap: &Heap, items: &[Value], out: &mut Vec<Symbol>) {
    let mut optional = false;
    for &item in items {
        match item {
            Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => optional = true,
            Value::Sym(s) if value::symbol_is(s, kw::AMP) => optional = false,
            _ => match list_items(heap, item).as_deref() {
                Some(&[slot, _default]) if optional => collect_pattern_binders(heap, slot, out),
                _ => collect_pattern_binders(heap, item, out),
            },
        }
    }
}

/// How many times `sym` occurs in `pattern`. A name written twice in one pattern is an
/// equality constraint (`[x x]`), and a `bytes` segment's size may name an earlier
/// segment's binder — either way the pattern itself reads it.
fn occurrences_in(heap: &Heap, pattern: Value, sym: Symbol) -> usize {
    match pattern {
        Value::Sym(s) if s == sym => 1,
        Value::Vector(id) => heap
            .vector(id)
            .to_vec()
            .into_iter()
            .map(|item| occurrences_in(heap, item, sym))
            .sum(),
        Value::Pair(_) => list_items(heap, pattern)
            .unwrap_or_default()
            .into_iter()
            .map(|item| occurrences_in(heap, item, sym))
            .sum(),
        Value::Map(id) => heap
            .map_entries(id)
            .iter()
            .map(|&(key, entry)| occurrences_in(heap, key, sym) + occurrences_in(heap, entry, sym))
            .sum(),
        _ => 0,
    }
}

/// **Unused binders at every other binding form**, on the un-expanded tree. Each of these
/// lowers to `match*` or to synthetic `let`s, which the main walk's unused-`let` lint
/// exempts by design, so before this a binder nothing read was reported only when it was
/// a plain symbol in a `let`/`letrec`. Two of the cases are bugs, not tidiness:
/// `(fold-for (acc 0 x xs) x)` never folds, and a `match` arm that binds a field and then
/// reads the whole value has usually read the wrong name.
///
/// `let`/`letrec` lint only their destructuring patterns here: a symbol binder there is
/// the main lint's, and reporting it twice would be noise.
fn lint_binding_form(
    heap: &Heap,
    head: Symbol,
    form: Value,
    items: &[Value],
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if heap.is_synthetic(form) {
        return;
    }
    let name = value::symbol_name_ref(head);
    match name {
        "let" | "letrec" | "with" | "if-let" | "when-let" | "if-some" | "when-some" => {
            let noun = "unused let binding";
            lint_sequential_binders(heap, form, items, name == "letrec", name, noun, out);
        }
        "dolist" | "dotimes" | "doseq" | "for" | "fold-for" => {
            lint_sequential_binders(heap, form, items, false, name, "unused loop binding", out);
        }
        "match" | "receive" => {
            let clauses = if name == "match" {
                items.get(2..)
            } else {
                items.get(1..)
            };
            for &clause in clauses.unwrap_or_default() {
                let Some(clause_items) = list_items(heap, clause) else {
                    continue;
                };
                let Some((&pattern, scope)) = clause_items.split_first() else {
                    continue;
                };
                // `receive`'s timeout clause, `(after ms body…)`, binds nothing.
                if name == "receive"
                    && matches!(pattern, Value::Sym(s) if value::symbol_is(s, "after"))
                {
                    continue;
                }
                report_unused_binders(heap, clause, pattern, scope, "unused pattern binding", out);
            }
        }
        "catch" => {
            if let Some((&pattern, scope)) = items.get(1..).and_then(|rest| rest.split_first()) {
                report_unused_binders(heap, form, pattern, scope, "unused catch binding", out);
            }
        }
        "as->" => {
            if let (Some(&pattern), Some(scope)) = (items.get(2), items.get(3..)) {
                report_unused_binders(heap, form, pattern, scope, "unused let binding", out);
            }
        }
        _ => {}
    }
}

/// The `(pattern expr pattern expr …)` binding list of `items[1]`, each pattern visible to
/// every later element and the body (`items[2..]`), and for `letrec` to the earlier ones
/// too. A keyword in a pattern position is a clause (`:when guard`, `:into coll`) whose
/// form is an expression, read like any other.
fn lint_sequential_binders(
    heap: &Heap,
    form: Value,
    items: &[Value],
    letrec: bool,
    name: &str,
    noun: &str,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let (Some(&binds), Some(body)) = (items.get(1), items.get(2..)) else {
        return;
    };
    let Some(binds) = heap.list_or_vector_items(binds) else {
        return;
    };
    // An all-symbol `let`/`letrec` stays a plain let the main lint reads. One destructured
    // pattern lowers the WHOLE list to synthetic lets it exempts, the plain names included.
    let symbols_are_the_main_lints = (name == "let" || name == "letrec")
        && binds
            .iter()
            .step_by(2)
            .all(|pattern| matches!(pattern, Value::Sym(_)));
    let mut position = 0;
    while position < binds.len() {
        let pattern = binds[position];
        let is_clause = matches!(pattern, Value::Keyword(_));
        let is_main_lints = symbols_are_the_main_lints && matches!(pattern, Value::Sym(_));
        if !is_clause && !is_main_lints {
            let earlier = if letrec { &binds[..position] } else { &[][..] };
            let later = binds.get(position + 2..).unwrap_or_default();
            let scope: Vec<Value> = earlier.iter().chain(later).chain(body).copied().collect();
            report_unused_binders(heap, form, pattern, &scope, noun, out);
        }
        position += 2;
    }
}

/// Warn on each binder of `pattern` that `scope` never reads, at `pattern`'s position or,
/// for a bare symbol (which has none), `located`'s. The exemptions are the other unused
/// lints': the `_` prefix, gensyms, a name that shadows a global (a deliberate isolation
/// test, which `_` cannot express), and a name the pattern itself reads.
fn report_unused_binders(
    heap: &Heap,
    located: Value,
    pattern: Value,
    scope: &[Value],
    noun: &str,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(pos) = heap
        .form_pos_only(pattern)
        .or_else(|| heap.form_pos_only(located))
    else {
        return;
    };
    let mut binders = Vec::new();
    collect_pattern_binders(heap, pattern, &mut binders);
    for binder in binders {
        let spelling = value::symbol_name_ref(binder);
        if spelling.starts_with('_')
            || walk::is_gensym_sym(binder)
            || sigs::is_globally_bound(heap, binder)
            || sigs::has_curated_sig(binder)
            || occurrences_in(heap, pattern, binder) > 1
        {
            continue;
        }
        if !scope.iter().any(|&f| walk::sym_appears_in(heap, f, binder)) {
            out.push((Some(pos), format!("{noun}: {spelling}")));
        }
    }
}

/// Walk `form` (un-expanded) for inline pattern-clause `fn` literals and record each
/// one's `(heads, tail)` arms in `ctx` under its printed clause-head-list key — the side
/// channel [`sigs::clause_lambda_ret`] reads back at call sites, where the expanded tree
/// holds only the lowered variadic `match*` lambda (see `Ctx::fn_literal_clauses`).
/// Quoted/quasiquoted subtrees are data/templates and `comment` never runs — skipped,
/// like the other surface passes.
fn record_fn_literal_clauses(heap: &Heap, form: Value, ctx: &mut Ctx) {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
            Value::Pair(_) => {
                let Some(items) = list_items(heap, v) else {
                    continue;
                };
                if let Some(&Value::Sym(head)) = items.first() {
                    if value::symbol_is(head, kw::QUOTE)
                        || value::symbol_is(head, kw::QUASIQUOTE)
                        || value::symbol_is(head, kw::COMMENT)
                    {
                        continue;
                    }
                    if walk::is_fn_head(head)
                        && let Some((key, arms, printed)) = pattern_fn_literal(heap, &items)
                    {
                        ctx.add_fn_literal_clauses(key, arms, printed);
                    }
                }
                work.extend(items);
            }
            _ => {}
        }
    }
}

/// Read `items` (a surface `(fn …)` form's items) as a literal the expander will lower
/// to `match*` — clause style with at least one pattern head or guard, or the plain
/// single-clause style with a destructuring parameter. `None` for anything the expander
/// keeps as a native `fn` (plain params, arity-only clauses), which the direct path in
/// `clause_lambda_ret` / `lambda_ret` already types. Returns the head-list key (printed
/// exactly as the lowering's `(quote heads)` datum prints), the `(heads, tail)` arms,
/// and the full printed clause list for the registry's collision test.
#[allow(clippy::type_complexity)]
fn pattern_fn_literal(
    heap: &Heap,
    items: &[Value],
) -> Option<(String, Vec<(Vec<Value>, Value)>, String)> {
    let forms = match items.get(1..) {
        Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
        Some(rest) => rest,
        None => return None,
    };
    if forms.is_empty() {
        return None;
    }
    let is_pattern_head = |h: &Value| !matches!(h, Value::Sym(_));
    let print = |v: Value| crate::syntax::printer::print(heap, v);
    // Clause style: every form is `(head-list body…)`.
    let as_clauses: Option<Vec<_>> = forms.iter().map(|&c| sigs::clause_arm(heap, c)).collect();
    if let Some(arms) = as_clauses {
        let lowered = arms
            .iter()
            .any(|a| a.guard.is_some() || a.heads.iter().any(is_pattern_head));
        if !lowered {
            return None; // arity-only clauses stay native `fn` clauses
        }
        let head_lists: Vec<String> = forms
            .iter()
            .map(|&c| Some(print(*list_items(heap, c)?.first()?)))
            .collect::<Option<_>>()?;
        let key = format!("({})", head_lists.join(" "));
        let printed: String = forms
            .iter()
            .map(|&c| print(c))
            .collect::<Vec<_>>()
            .join(" ");
        let out = arms
            .into_iter()
            .map(|a| a.body.last().map(|&t| (a.heads, t)))
            .collect::<Option<Vec<_>>>()?;
        return Some((key, out, printed));
    }
    // Plain single-clause style with a destructuring parameter: `(fn ([k v]) body…)`.
    let plist = *forms.first()?;
    let heads = match plist {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, plist)?,
        _ => return None,
    };
    if !heads.iter().any(is_pattern_head) {
        return None; // plain params — native `fn`, `lambda_ret`'s case
    }
    let tail = *forms.get(1..)?.last()?;
    let key = format!("({})", print(plist));
    let printed = format!("{} {}", print(plist), print(tail));
    Some((key, vec![(heads, tail)], printed))
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

/// Warn when one file binds the same top-level name twice in one module: the
/// later `def`/`defn`/`defn-`/`defmacro`/`defdyn`/`defrecord` silently replaces the
/// earlier, so the first is dead code that still reads as the definition. Found the hard
/// way: `project.blsp` carried a public sorted `source-files` and, 1,300 lines later, a
/// private unsorted one — every caller ran the second while the first's docstring made
/// the sortedness promise. Nothing said so: the cross-file duplicate-global lint is
/// per-namespace across files, and one file's rebinding of its own name is legal.
///
/// Reads the *un-expanded* forms, descending `(do …)` and `(check-allow …)` wrappers the
/// way the sig collector does. Region-aware (ADR-223): a `(defmodule …)` starts a fresh
/// name set, so two modules in one file may each bind `helper`. Suppress a deliberate
/// override with `(check-allow :duplicate-def …)` around the second definition.
fn lint_duplicate_defs(heap: &mut Heap, forms: &[Value], out: &mut Vec<(Option<Pos>, String)>) {
    let mut seen: HashMap<Symbol, Option<Pos>> = HashMap::new();
    for &form in forms {
        lint_duplicate_defs_in(heap, form, false, &mut seen, out);
    }
}

fn lint_duplicate_defs_in(
    heap: &mut Heap,
    form: Value,
    allowed: bool,
    seen: &mut HashMap<Symbol, Option<Pos>>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    // Recurses through nested `(do (do …))`, like `collect_register_sig_forms` beside it,
    // and for the same reason grows the stack in heap-backed segments: a 30 000-deep
    // legal chain (`checker_survives_pathologically_deep_forms`) overflowed the native
    // stack here — the one walker of the un-expanded tree that lacked the guard, and a
    // SIGSEGV `catch_unwind` cannot catch.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        lint_duplicate_defs_in_inner(heap, form, allowed, seen, out)
    })
}

fn lint_duplicate_defs_in_inner(
    heap: &mut Heap,
    form: Value,
    allowed: bool,
    seen: &mut HashMap<Symbol, Option<Pos>>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Some(items) = heap.list_items(form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    if value::symbol_is(head, kw::DEFMODULE) {
        seen.clear();
        return;
    }
    if value::symbol_is(head, kw::DO) {
        for &it in &items[1..] {
            lint_duplicate_defs_in(heap, it, allowed, seen, out);
        }
        return;
    }
    if value::symbol_is(head, "check-allow") || value::symbol_is(head, "%lint-allow") {
        let here = allowed
            || matches!(items.get(1), Some(&Value::Keyword(k)) if value::symbol_is(k, "duplicate-def"));
        for &it in &items[2..] {
            lint_duplicate_defs_in(heap, it, here, seen, out);
        }
        return;
    }
    let is_def = [
        kw::DEF,
        kw::DEFN,
        kw::DEF_PRIVATE,
        kw::DEFN_PRIVATE,
        kw::DEFMACRO,
        kw::DEFDYN,
        kw::DEFRECORD,
    ]
    .iter()
    .any(|k| value::symbol_is(head, k));
    if !is_def {
        return;
    }
    let Some(&Value::Sym(name)) = items.get(1) else {
        return;
    };
    let pos = heap.form_pos_only(form);
    match seen.get(&name) {
        Some(first) if !allowed => {
            let where_first = first
                .map(|p| format!(" at line {}", p.line))
                .unwrap_or_default();
            out.push((
                pos,
                format!(
                    "`{}` is defined twice in this file: this definition replaces the one{where_first}, \
                     which is now dead code — remove one, or wrap a deliberate override in \
                     (check-allow :duplicate-def …)",
                    value::symbol_name(name)
                ),
            ));
        }
        _ => {
            seen.insert(name, pos);
        }
    }
}

/// Validate this file's hand-written `(sig …)` declarations (the declaration check) — see the
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
        // Typed parameters (ADR-420) are a declaration too: each type they write must read,
        // or the position silently widens to `any`.
        if let Some((name, types)) = crate::eval::macros::defn_param_types(heap, form) {
            let spelling = value::symbol_name(name);
            let pos = heap.form_pos_only(form);
            for (label, ty_form) in types {
                if let Some(problem) = annot::type_expr_problem(heap, ty_form) {
                    let what = if label == "->" {
                        "the result".to_string()
                    } else {
                        format!("parameter `{label}`")
                    };
                    out.push((pos, format!("defn {spelling}: {what}: {problem}")));
                }
            }
            continue;
        }
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
        let qualified = qualify_decl_name(heap, ctx, file_ns, name);
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
            let declared = walk::arity_of_sig(&sig);
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
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_register_sig_forms_inner(heap, form, out)
    })
}

fn collect_register_sig_forms_inner(heap: &mut Heap, form: Value, out: &mut Vec<Value>) {
    let Some(items) = heap.list_items(form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    // `(do …)` and `(%lint-allow :category …)` — what `check-allow` expands to — both wrap
    // definitions; a `(sig …)` inside a `check-allow` block must still declare its `defn`.
    if crate::core::value::symbol_is(head, kw::DO)
        || crate::core::value::symbol_is(head, "%lint-allow")
    {
        for &it in &items[1..] {
            collect_register_sig_forms(heap, it, out);
        }
        return;
    }
    if crate::core::value::symbol_is(head, "%register-sig") && items.len() == 3 {
        // items[1] = (quote name), items[2] = (quote type)
        if let (Some(name), Some(ty)) =
            (walk::unquote(heap, items[1]), walk::unquote(heap, items[2]))
        {
            let sig_head = crate::core::value::sym("sig");
            let rebuilt = heap.list(vec![sig_head, name, ty]);
            out.push(rebuilt);
        }
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
/// the alias macro-expand to `mod/…`). Used for the file's world (B8) and the
/// whole-project require edges — a module reached by any of these is genuinely required. Kept separate from
/// [`extract_use_module_names`], whose `:use`-only view backs the unused-import lint.
fn extract_import_module_names(heap: &Heap, forms: &[Value]) -> Vec<String> {
    extract_clause_modules(heap, forms, &["use", "use-internals", "alias", "load"])
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
            if let Some(items) = list_items(heap, clause)
                && let Some(Value::Keyword(kw_sym)) = items.first()
                && keywords.iter().any(|k| value::symbol_is(*kw_sym, k))
            {
                if value::symbol_is(*kw_sym, "load") {
                    // `(:load a b …)` names several modules (ADR-335); every symbol
                    // after the keyword is one. The other clauses take one module
                    // and may carry a symbol that is NOT a module (`:alias m :as short`).
                    for item in items.iter().skip(1) {
                        if let Value::Sym(mod_sym) = item {
                            result.push(value::symbol_name(*mod_sym).to_string());
                        }
                    }
                } else if let Some(&Value::Sym(mod_sym)) = items.get(1) {
                    result.push(value::symbol_name(mod_sym).to_string());
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
/// dynamic require whose module argument isn't a literal symbol/string.
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

/// The module prefixes the file makes reachable *itself*: every `(:use M)` in its
/// header, every `(require 'M)` **anywhere** in the file (including one nested in a
/// function body — `project.blsp` requires `test`/`package` lazily inside the functions
/// that use them, to keep startup lean), and its own namespace. A file that requires `M`
/// *somewhere* is treated as reaching `M` for the whole file — an over-approximation.
/// Direct requires only, not their transitive closure. The seed of the file's world
/// (B8, `check_file_mode`).
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
    // `(defmodule …)`) is trivially reachable — an intra-file qualified reference to a
    // co-located sibling module is in the file's world. `file_ns` above
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

/// Extend `mods` with every module reachable from it through the standard library's own
/// require edges (B8). A baked-in module's direct requires are read from its
/// embedded source once per process (`STD_REQUIRES`); a name that is not a baked-in module
/// is a leaf here (the whole-project driver has already closed the project's own edges).
fn close_through_std(heap: &mut Heap, mods: &mut HashSet<String>) {
    static STD_REQUIRES: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<String>>>> =
        std::sync::OnceLock::new();
    let cache = STD_REQUIRES.get_or_init(Default::default);
    let mut frontier: Vec<String> = mods.iter().cloned().collect();
    while let Some(name) = frontier.pop() {
        let cached = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&name)
            .cloned();
        let direct = match cached {
            Some(direct) => direct,
            None => {
                let direct = match crate::builtins::modules::embedded_module_source(&name) {
                    Some(source) => crate::syntax::reader::read_all(heap, source)
                        .map(|forms| module_direct_requires(heap, &forms).1)
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(name.clone(), direct.clone());
                direct
            }
        };
        for dep in direct {
            if mods.insert(dep.clone()) {
                frontier.push(dep);
            }
        }
    }
}

/// A file's own module name (from its `(defmodule …)` header, or `None`) and the module
/// names it **directly** pulls in — `(:use …)` / `(:use-internals …)` clauses plus every
/// `(require 'M)` anywhere in the file. The edge list the whole-project driver
/// (`std/tool/project.blsp`) closes transitively into each file's require-closure, then
/// feeds back to [`check_file_ext`] as the seed of the file's world (B8). Own ns is excluded from `requires`.
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
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
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
/// (includes quoted data and binder positions): it only ever WIDENS the file's world.
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
/// compile made a whole-project check O(files²).
///
/// Mirrors `defmodule`'s `(:use)`/`(:alias)` expansion (std/prelude.blsp) + the `%refer`/
/// `%alias` builtins exactly: `(:use mod)` refers mod's public (non-`--`) names bare;
/// `(:use mod :only [a b])` / `:refer` refers just those; `(:alias mod [:as short])` adds a
/// `short/` → `mod` prefix alias. A used module that isn't loaded (a bare-file check outside a
/// project) is `require`d first — rare, and correctness there beats the O(files²) speed win.
/// The modules a `(defmodule …)` header brings into scope, as the type-alias resolver
/// needs them (ADR-327): the ROOTED names of every `:use`d module —
/// a bare alias name resolves to one of theirs before the loaded-module-wide unique-suffix
/// rule — and each `(:alias mod :as short)` prefix, so `short/name` in a `sig` reaches
/// `mod/name`. Accumulated across a file's headers (the region model, ADR-223).
#[derive(Default)]
pub(crate) struct ImportScope {
    pub(crate) used: Vec<String>,
    pub(crate) aliases: Vec<(String, String)>,
    /// `(:use mod :only [name])` naming something `mod` does not define — the check the
    /// runtime `%refer` cannot make (KI-211: at refer time a module may be mid-load or
    /// image-backed), reported at the `(:use …)` clause.
    pub(crate) unknown_only: Vec<(Option<Pos>, String)>,
}

impl ImportScope {
    fn extend(&mut self, other: ImportScope) {
        self.used.extend(other.used);
        self.aliases.extend(other.aliases);
        self.unknown_only.extend(other.unknown_only);
    }
}

fn setup_check_imports(heap: &mut Heap, header: Value) -> ImportScope {
    let mut scope = ImportScope::default();
    // PASS A — parse the clauses into GC-STABLE data (`Symbol`s are Copy `u32`) with NO
    // eval, so nothing LOCAL is held across the `require` eval in pass B (which can collect
    // and relocate handles). Holding a parsed `Value` across that eval was a use-after-GC.
    enum Clause {
        // (module, Some(:only subset) | None = all public, the clause's position)
        Use(Symbol, Option<Vec<Symbol>>, Option<Pos>),
        UseExcept(Symbol, Vec<Symbol>), // (:use mod :exclude [names]) — all public minus names
        Alias(Symbol, Symbol),          // (short-prefix-name, module)
        UseInternals(Symbol),           // (:use-internals mod) — ADR-146 grant
        Shadow(Symbol),                 // (:shadow name …) — a declared root-macro shadow, ADR-423
    }
    let mut clauses: Vec<Clause> = Vec::new();
    {
        let Some(items) = list_items(heap, header) else {
            return scope;
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
                        let pos = heap.form_pos_only(*clause);
                        clauses.push(Clause::Use(mod_sym, Some(names), pos));
                    } else {
                        clauses.push(Clause::UseExcept(mod_sym, names));
                    }
                } else {
                    clauses.push(Clause::Use(mod_sym, None, None));
                }
            } else if value::symbol_is(*kw_sym, "use-internals") {
                let Some(&Value::Sym(mod_sym)) = citems.get(1) else {
                    continue;
                };
                // A grant is also a use (publics refer bare), plus the ADR-146
                // internals key the privacy walk consults.
                clauses.push(Clause::Use(mod_sym, None, None));
                clauses.push(Clause::UseInternals(mod_sym));
            } else if value::symbol_is(*kw_sym, "shadow") {
                for name in citems.iter().skip(1) {
                    if let Value::Sym(name) = *name {
                        clauses.push(Clause::Shadow(name));
                    }
                }
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
    for clause in clauses {
        match clause {
            Clause::Use(mod_sym, subset, pos) => {
                // Package-rooted namespaces (ADR-070): an intra-package `(:use b)` — in a
                // dependency, or in the root project under its `:name` — roots to `pkg/b`.
                // The runtime `%refer` target roots via `%root-module-name`; the checker
                // must root the same way here or it scans `b/*` for exports that actually
                // live under `pkg/b/*` and flags every imported name unbound. External /
                // std / already-qualified names are left unchanged.
                let mod_sym = heap.root_module_name(mod_sym);
                let mod_name = value::symbol_name(mod_sym);
                scope.used.push(mod_name.clone());
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
                        // What the module defines, for naming an `:only` entry it does not
                        // (KI-211's follow-up). A module with no exports here is one the
                        // check could not load — say nothing about it rather than guess.
                        let exports: std::collections::HashSet<Symbol> =
                            deps::obs_module_exports(heap, &prefix)
                                .into_iter()
                                .map(|(bare, _)| bare)
                                .collect();
                        for bare in names {
                            let bare_name = value::symbol_name(bare);
                            let qual = value::intern(&format!("{}/{}", mod_name, bare_name));
                            if heap.is_private(qual) && !granted {
                                continue;
                            }
                            let defined = exports.contains(&bare)
                                || sigs::is_globally_bound(heap, qual)
                                || crate::eval::derive::image_sig_arity(heap, qual).is_some()
                                || protocol::ability_declared_by(heap, bare, &mod_name);
                            if !defined && !exports.is_empty() {
                                scope.unknown_only.push((
                                    pos,
                                    format!(
                                        "(:use {mod_name} :only [... {bare_name} ...]): \
                                         {mod_name} defines no public `{bare_name}`"
                                    ),
                                ));
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
                scope.used.push(mod_name.clone());
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
            Clause::Shadow(name) => {
                // What the runtime clause's `%declare-shadow` records, so checking the module
                // compiles it as loading does — without the shadow warning it declared away.
                let key = crate::eval::macros::shadow_declared_key(&value::symbol_name(name));
                heap.add_import(key, name);
            }
            Clause::Alias(short, mod_sym) => {
                // Root the alias TARGET (ADR-070) so `short/name` resolves to `pkg/mod/name`;
                // the local `short` prefix is unchanged.
                let mod_sym = heap.root_module_name(mod_sym);
                // The runtime clause `require-one`s the target; the checker must too, or the
                // module's `deftype`s are not in the alias table when a `sig` says `short/name`
                // (a CALL `short/name` infers its load lazily, ADR-227 — a type name cannot).
                ensure_loaded(heap, mod_sym);
                let key = value::intern(&format!("{}/", value::symbol_name(short)));
                heap.add_import(key, mod_sym);
                scope
                    .aliases
                    .push((value::symbol_name(short), value::symbol_name(mod_sym)));
            }
        }
    }
    scope
}

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
                    // function, which ADR-259's the declaration check then rejects as contradicting
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
    let Some(arms) = ctx.inferred_overload(name).filter(|a| !a.is_empty()) else {
        return single;
    };
    // Parameters: the position-wise union over every arm of the FIRST arm's arity — the
    // shape a reader is looking at, and a sound over-approximation of what those arms
    // admit. Return: the union of the arms' returns when the clause inference typed them,
    // else the params-less union the expanded tree gave.
    let arity = arms[0].params.len();
    let same_arity: Vec<&Sig> = arms.iter().filter(|a| a.params.len() == arity).collect();
    let params: Vec<Ty> = (0..arity)
        .map(|i| {
            same_arity
                .iter()
                .fold(Ty::NEVER, |acc, a| acc.union(a.params[i].clone()))
        })
        .collect();
    let arm_ret = arms
        .iter()
        .fold(Ty::NEVER, |acc, a| acc.union(a.ret.clone()));
    let ret = match &single {
        _ if !arm_ret.is_any() => arm_ret,
        Some(s) => s.ret.clone(),
        None => arm_ret,
    };
    Some(Sig::new(params, ret))
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

/// The most rounds the joint fixpoint of caller-derived parameters and returns runs
/// before it gives up (and says so).
const MAX_JOINT_ROUNDS: usize = 32;

/// The most rounds the same-file return inference runs before the names still moving
/// fall back to an `any` return (and the file says so).
const MAX_RETURN_ROUNDS: usize = 16;

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
    let (forms, expanded, import_scope) = expand_and_load(heap, forms, roots_base, out);
    let (mut ctx, visible) =
        collect_file_scope(heap, &forms, &expanded, extra_required, strict, file_ns);
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
/// module load can collect at any depth and relocate them), plus the header's import
/// scope.
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
) -> (Vec<Value>, Vec<Value>, ImportScope) {
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
    let forms: Vec<Value> = (0..n).map(|j| heap.root_at(roots_base + j)).collect();
    let expanded: Vec<Value> = (0..n).map(|j| heap.root_at(roots_base + n + j)).collect();
    (forms, expanded, import_scope)
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

/// The type vocabulary a `sig` may name, installed before any sig is parsed: sealed
/// abilities as types (ADR-181, so `(sig f (Shape -> …))` resolves to the union of its
/// members' shapes), which ids are records (the tiebreak for a sealed member that also
/// spells a built-in kind), the type aliases (ADR-326), and the operator sugar's domains
/// (ADR-299).
fn install_type_vocabulary(
    heap: &Heap,
    expanded: &[Value],
    file_ns_name: Option<&str>,
    mut import_scope: ImportScope,
    multi_info: &protocol::MultiInfo,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    annot::set_ability_types(protocol::ability_type_table(heap, expanded));
    annot::set_record_ids(protocol::record_id_names(heap, expanded));
    out.append(&mut import_scope.unknown_only);
    annot::set_type_aliases(
        protocol::type_alias_table(heap, expanded, file_ns_name),
        file_ns_name.map(str::to_string),
        import_scope,
    );
    // A `deftype` whose shape exceeds the lattice's node budget (`MAX_TY_NODES`) is
    // widened to its bare tags by the constructors, silently: every field reads as
    // unrefined. A written shape that came back with no fields (or no positions) is
    // that case exactly; say so at the declaration.
    for &form in expanded {
        let mut aliases = HashMap::new();
        protocol::collect_register_types_into(heap, form, None, &mut aliases);
        let mut aliases: Vec<(String, Value)> = aliases.into_iter().collect();
        aliases.sort_by(|a, b| a.0.cmp(&b.0)); // report order, not hash order (B8)
        for (name, ty_form) in aliases {
            let Some(ty) = annot::parse_type(heap, ty_form) else {
                continue;
            };
            let declares_shape = list_items(heap, ty_form)
                    .and_then(|items| items.first().copied())
                    .is_some_and(|h| matches!(h, Value::Sym(s) if value::symbol_is(s, "record") || value::symbol_is(s, "tuple")));
            if declares_shape && ty.record_fields().is_none() && ty.positional_elems().is_none() {
                out.push((
                        heap.form_pos_only(form),
                        format!(
                            "deftype {name}: the shape exceeds the checker's budget of {} nodes and is read as a bare `{}` — every field is unrefined; split it into named parts",
                            crate::types::MAX_TY_NODES,
                            ty
                        ),
                    ));
            }
        }
    }
    // The operator sugar's domains: `number` plus the records `num/*` / `compare-to`
    // have methods for, from this file's `defmethod`s + the registry.
    sigs::set_operator_domains(protocol::operator_domains(multi_info));
}

/// Register the file's `(sig …)` declarations — the authoritative signatures the call
/// check consults first — and build its ability facts. Two sources of declarations, fed
/// through the same parsers and namespace qualification (`register_declared_sig`):
///
///  (a) the *un-expanded* top-level `(sig …)` forms, where a hand-written declaration
///      is legible (the `sig` macro's own output is dropped from the analysed tree); and
///  (b) every `(%register-sig 'name 'type)` in the *expanded* tree — what a
///      macro-emitted sig lowers to (`defrecord` nests its sigs in a `(do …)` that (a)
///      cannot see). The overlap with (a) re-registers idempotently.
///
/// A declared name is qualified to the file's namespace exactly as the resolve pass
/// qualifies a def head, so `(sig g …)` inside `(defmodule ns …)` keys under `ns/g`.
fn register_declarations(
    heap: &mut Heap,
    forms: &[Value],
    expanded: &[Value],
    ctx: &mut Ctx,
    file_ns_name: Option<&str>,
    visible: &HashSet<String>,
) -> std::sync::Arc<protocol::AbilityInfo> {
    // Building a `(sig name type)` form needs `&mut heap`, so collect first and
    // register after — GC is blocked for the whole check, so the handles stay live.
    let mut macro_sig_forms: Vec<Value> = Vec::new();
    for &form in expanded {
        collect_register_sig_forms(heap, form, &mut macro_sig_forms);
    }
    // THIS file's records' declared field types, BEFORE any other sig is parsed: a
    // `(sig f (pt -> …))` resolves the record NAME to a shape carrying them
    // (`annot::record_ty`), and the file is not loaded while it is checked, so the
    // constructor's sig — where `defrecord` puts the field types — is nowhere else.
    // A constructor's sig is the one whose name IS a registered record id and whose
    // result is a record shape.
    {
        let mut field_types = HashMap::new();
        for &form in &macro_sig_forms {
            let Some((name, sig)) = annot::parse_sig_decl(heap, form) else {
                continue;
            };
            let id = value::symbol_name(qualify_decl_name(heap, ctx, file_ns_name, name));
            if let Some(fields) = sig.ret.record_fields() {
                field_types.entry(id).or_insert_with(|| fields.clone());
            }
        }
        annot::set_record_field_types(field_types);
    }
    // The ability facts and the sealed-op occurrence-typing domains go in before any
    // pass runs `sig_of`, so an imported function's inferred sig is never cached
    // without the sealed-op demand (ADR-190). Restricted to the file's world (B8):
    // another file's imported abilities, left in the process registry by checking it
    // first, are none of this file's business.
    let ability_info = std::sync::Arc::new(protocol::build_ability_info_visible(
        heap,
        expanded,
        Some(visible),
    ));
    annot::set_sealed_op_domains(protocol::build_sealed_op_domains(&ability_info));
    // On the ctx from here too: the demand walk asks `is_ability_op` to tell an op
    // function from a same-file function spelling its name (ADR-350).
    ctx.set_ability(ability_info.clone());
    for &form in forms.iter().chain(&macro_sig_forms) {
        register_declared_sig(heap, ctx, file_ns_name, form);
    }
    ability_info
}

/// The value type of each *undeclared* global defined exactly once by `(def g
/// <non-fn-expr>)`: the RHS's `expr_ty` becomes `g`'s current-image type, read as
/// `dynamic_within` (the `∩` relation — reload-safe, warns only on provable
/// disjointness). Skipped: a global defined more than once, a macro, a dynamic or
/// earmuffed variable (rebound over its lifetime), and a function value (its arrow is
/// inferred separately).
///
/// Reads `top_level_defs`, not `expanded`, so the `def` inside a `def-`'s `(do (def g …)
/// (%mark-private 'g))` is seen.
fn infer_global_value_types(heap: &Heap, expanded: &[Value], ctx: &mut Ctx) {
    let def_count = def_counts(heap, expanded);
    for form in top_level_defs(heap, expanded) {
        let Some((name, rhs)) = def_name_and_value(heap, form) else {
            continue;
        };
        // An earmuffed `*name*` is dynamic by convention (the lazy-init `(when (nil? *g*)
        // (def *g* …))` pattern), so pinning it to its load-time value would false-positive
        // once reassigned — and flow into the return of every function that reads it.
        if value::is_gensym(name)
            || def_count.get(&name) != Some(&1)
            || ctx.is_file_macro(name)
            || heap.is_dynamic(name)
            || infer::is_earmuffed(name)
        {
            continue;
        }
        if let Some(ty) = infer::expr_ty(heap, rhs, ctx)
            && !ty.contains_tag(value::Tag::Fn)
            && !ty.contains_tag(value::Tag::Native)
        {
            ctx.add_inferred_value_ty(name, ty);
        }
    }
}

/// How many times each name is `def`'d anywhere in the expanded tree.
fn def_counts(heap: &Heap, expanded: &[Value]) -> HashMap<Symbol, usize> {
    let mut counts = HashMap::new();
    for &form in expanded {
        count_defs(heap, form, &mut counts);
    }
    counts
}

/// The functions whose signatures the file infers: every top-level `(def name rhs)`
/// defined once, undeclared, and not a macro, dynamic variable or gensym.
fn inference_candidates(heap: &Heap, expanded: &[Value], ctx: &Ctx) -> Vec<(Symbol, Value)> {
    let def_count = def_counts(heap, expanded);
    top_level_defs(heap, expanded)
        .into_iter()
        .filter_map(|form| {
            let (name, rhs) = def_name_and_value(heap, form)?;
            (def_count.get(&name) == Some(&1)
                && !ctx.is_file_macro(name)
                && !heap.is_dynamic(name)
                && !value::is_gensym(name)
                && (ctx.declared_sig(name).is_none() || ctx.is_open_ret(name)))
            .then_some((name, rhs))
        })
        .collect()
}

/// **Same-file function inference.** The file being checked isn't loaded, so `sig_of`'s
/// loaded-closure inference can't see its own `(defn …)`s. Infer each candidate's return
/// from its FORM (`sigs::infer_return_from_form`) and record it, with its inferred
/// parameter demands (ADR-190, which under-constrain, so a flagged argument genuinely
/// errors), so a same-file call flows the result as a loaded-function call does.
///
/// A bounded fixpoint resolves callees leaf-up: `infer_return_from_form` defers until
/// every function its return depends on is recorded. Runs after the global value types,
/// which a body may read. Returns the candidates, which the caller-derived parameter
/// pass derives next.
fn infer_same_file_functions(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ctx: &mut Ctx,
    file_ns_name: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) -> Vec<(Symbol, Value)> {
    let derive_dbg = derive_debug();
    let candidates = inference_candidates(heap, expanded, ctx);
    // Each candidate's form stays reachable for call-site specialization
    // (`sigs::specialized_ret`): a call whose flat inferred return is `any` re-types the
    // body under the call's own argument types.
    for &(name, rhs) in &candidates {
        ctx.add_fn_form(name, rhs);
    }
    // A multi-clause `defn`'s clauses — read from the UN-EXPANDED form, the only place a
    // pattern- or `:when`-dispatched definition still has clauses (ADR-226) — go in BEFORE
    // the fixpoint: a same-file caller typed inside it needs the arms. The per-arm
    // signatures are derived again after the fixpoint, when every callee's return is final.
    let mut clause_defs: Vec<(Symbol, Vec<Value>)> = Vec::new();
    for &form in forms {
        if let Some((name, clauses)) = defn_clauses(heap, form) {
            let qualified = qualify_decl_name(heap, ctx, file_ns_name, name);
            if let Some(arms) = sigs::infer_overload_from_clauses(heap, &clauses, ctx) {
                ctx.add_inferred_overload(qualified, arms);
            }
            ctx.add_clause_arms(qualified, clauses.clone());
            clause_defs.push((qualified, clauses));
        }
    }
    // Inline PATTERN-clause `fn` literals get the same surface record, keyed by their
    // printed clause-head list: the expanded tree holds only the lowered `match*` lambda,
    // whose embedded `(%match-error … (quote heads))` call is how
    // `sigs::clause_lambda_ret` finds its way back here from a combinator call site.
    for &form in forms {
        record_fn_literal_clauses(heap, form, ctx);
        lint_unused_pattern_binders(heap, form, out);
    }
    // The names still moving when the bound ends the ascent: a value on an ascending chain
    // cut short is an UNDER-approximation, so each falls back to its demands with an `any`
    // return (sound), and the file says so.
    let mut still_moving: Vec<Symbol> = Vec::new();
    let mut older_returns: HashMap<Symbol, Ty> = HashMap::new();
    for round in 0..MAX_RETURN_ROUNDS {
        still_moving.clear();
        for &(name, rhs) in &candidates {
            let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), ctx) else {
                continue;
            };
            let current = ctx.inferred_fn_sig(name).map(|s| s.ret);
            let ret = ascend_return(ret, current.as_ref(), older_returns.get(&name), round);
            if let Some(current) = current.clone() {
                older_returns.insert(name, current);
            }
            if current != Some(ret.clone()) {
                // Parameters are body-derived (independent of this fixpoint, which only
                // resolves returns), so recomputing them is stable.
                let sig = match sigs::infer_params_from_form(heap, rhs, ctx) {
                    Some(demands) => demands.into_sig(ret),
                    None => crate::types::Sig::new(Vec::new(), ret),
                };
                if derive_dbg {
                    eprintln!(
                        "[derive] same-file return {} -> {}",
                        value::symbol_name_ref(name),
                        sig.ret
                    );
                }
                ctx.add_inferred_fn_sig(name, sig);
                still_moving.push(name);
            }
        }
        if still_moving.is_empty() {
            break;
        }
    }
    for &name in &still_moving {
        let rhs = candidates
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|&(_, rhs)| rhs);
        let sig = rhs
            .and_then(|rhs| sigs::infer_params_from_form(heap, rhs, ctx))
            .map(|demands| demands.into_sig(Ty::ANY))
            .unwrap_or_else(|| crate::types::Sig::new(Vec::new(), Ty::ANY));
        ctx.add_inferred_fn_sig(name, sig);
        out.push((
            rhs.and_then(|rhs| heap.form_pos_only(rhs)),
            format!(
                "checker gave up: the return of {} was still moving after {MAX_RETURN_ROUNDS} inference rounds — read as unknown here and by every caller",
                value::symbol_name_ref(name)
            ),
        ));
    }
    // A **multi-arm** candidate has no single signature, so record each arm's own; the
    // call check rules a call out only when every arity-relevant arm rejects it.
    for &(name, rhs) in &candidates {
        if let Some(arms) = sigs::infer_overload_from_form(heap, rhs, ctx) {
            ctx.add_inferred_overload(name, arms);
        }
    }
    // …and from the un-expanded `defn` clauses, now that every same-file return an arm's
    // tail may name has settled.
    for (qualified, clauses) in &clause_defs {
        if let Some(arms) = sigs::infer_overload_from_clauses(heap, clauses, ctx) {
            ctx.add_inferred_overload(*qualified, arms);
        }
    }
    // A candidate whose return stayed deferred (e.g. it returns an ability op) still gets
    // its parameter demands with an `any` return, so its callers are argument-checked.
    for &(name, rhs) in &candidates {
        if ctx.inferred_fn_sig(name).is_none()
            && let Some(demands) = sigs::infer_params_from_form(heap, rhs, ctx)
        {
            ctx.add_inferred_fn_sig(name, demands.into_sig(Ty::ANY));
        }
    }
    candidates
}

/// One step of a return's ascent (ADR-349/350), shared by both return fixpoints: an
/// interval that moved since the last round goes to its infinity, a value that nests a
/// previous one folds into a recursive type, and past the early rounds the rest is cut to
/// a depth — so `(cons x (self …))`, ⊥ in its first round, cannot lengthen forever.
fn ascend_return(mut ret: Ty, current: Option<&Ty>, older: Option<&Ty>, round: usize) -> Ty {
    if let Some(previous) = current {
        ret = ret.widen_intervals_against(previous);
    }
    for previous in current.into_iter().chain(older) {
        if let Some(folded) = Ty::fold_recursive(previous, &ret) {
            ret = folded;
            break;
        }
    }
    if round >= sigs::WIDEN_AFTER_ROUND {
        ret = ret.widened_below(sigs::WIDEN_DEPTH);
    }
    ret
}

/// **Caller-derived parameter types** (ADR-341). The union of what a function's call sites
/// in this file hand its parameters is a sound binding for the walk of its body, and for
/// the return its same-file callers read — where same-file inference bound the parameters
/// to their bottom-up DEMANDS alone (`(+ i 1)` says `number`; the callers say `int`). This
/// is what lets a leaf `sig` be enough: `int` flows from the public entry down through
/// every helper without a declaration on each.
///
/// Every candidate is derived, public or private: a derived type is a fact about THIS
/// FILE's calls, so a body warning under it says "every call in this file would fail here",
/// true whatever callers exist elsewhere; a caller in another file reads the demand-based
/// loaded inference.
///
/// The derivation and the returns feed each other (a helper's sharper return is a caller's
/// sharper argument), so this is a JOINT least fixpoint. Starting from the demand-based
/// returns, two helpers that hand each other an index through their returns could never
/// leave `number`, so the returns START at ⊥ and rise with the parameters (Kleene), and only
/// the fixpoint is used. A round that cannot type a return keeps the demand-based one. No
/// fixpoint within [`MAX_JOINT_ROUNDS`]: everything is restored as it was, and the file
/// says so.
fn derive_caller_params(
    heap: &Heap,
    expanded: &[Value],
    all_candidates: &[(Symbol, Value)],
    ctx: &mut Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    timing: &mut DeriveTiming,
) {
    let candidates: HashMap<Symbol, Value> = all_candidates.iter().copied().collect();
    if candidates.is_empty() {
        return;
    }
    let original: HashMap<Symbol, Option<Sig>> = candidates
        .keys()
        .map(|&name| (name, ctx.inferred_fn_sig(name)))
        .collect();
    // Only a LIVE function's return starts at ⊥. An escaped or site-less one is never
    // derived, and its in-file callers read its return while the others iterate: a ⊥ there
    // would make THEIR derived parameters under-approximate.
    let live = sigs::live_private_functions(heap, expanded, &candidates, ctx).unwrap_or_default();
    // The count relations between a live function's parameters that its sites establish
    // (ADR-350) — syntactic, so settled once here, for every round and for the walk.
    sigs::derive_count_aliases(heap, expanded, &candidates, &live, ctx);
    for (&name, sig) in &original {
        if let (Some(sig), true) = (sig, live.contains_key(&name)) {
            let mut floor = sig.clone();
            floor.ret = Ty::NEVER;
            ctx.add_inferred_fn_sig(name, floor);
        }
    }
    // A live function whose return never typed and is still at the ⊥ floor.
    let at_floor = |ctx: &Ctx, typed: &HashSet<Symbol>, name: Symbol, sig: &Sig| {
        !typed.contains(&name)
            && ctx.inferred_fn_sig(name).is_some_and(|s| s.ret.is_never())
            && !sig.ret.is_never()
    };
    let mut previous: HashMap<Symbol, Vec<Option<Ty>>> = HashMap::new();
    // Which returns a round has TYPED — as opposed to typed as `never`, which is a real
    // answer (every call raises) and must not be mistaken for a floor that never typed.
    let mut typed: HashSet<Symbol> = HashSet::new();
    let mut converged = false;
    // Whether anything the derivation reads moved since it last ran. When nothing did,
    // re-running it would reproduce `previous` exactly (it is a deterministic function of
    // the returns and its seed), so the round reuses it.
    let mut inputs_moved = true;
    // Each return's value from the round before last, for the fold (a value that recurses
    // through another function grows every other round).
    let mut older_returns: HashMap<Symbol, Ty> = HashMap::new();
    // The site walks, memoised per form across every round, with the names whose return or
    // derived parameters moved since the last derivation — `None` walks everything.
    let mut site_cache = sigs::SiteCache::new(heap, expanded, &candidates);
    let mut moved: Option<HashSet<Symbol>> = None;
    let mut derivation_declined = false;
    for round in 0..MAX_JOINT_ROUNDS {
        let round_started = std::time::Instant::now();
        let derivation_ran = inputs_moved;
        // A specialization is typed under the round's returns and derived parameters; the
        // memo of them must not outlive the round.
        sigs::clear_specializations();
        let derived = if inputs_moved {
            sigs::caller_derived_params(
                heap,
                expanded,
                &candidates,
                &live,
                ctx,
                &previous,
                &mut site_cache,
                moved.as_ref(),
            )
        } else {
            previous.clone()
        };
        timing.deriving += round_started.elapsed();
        timing.rounds += 1;
        // A derivation that declined leaves every parameter unknown for the rest of this
        // file: say so, once.
        if let Some(why) = sigs::take_derivation_decline()
            && !derivation_declined
        {
            derivation_declined = true;
            out.push((
                    None,
                    match why {
                        sigs::DeriveDecline::Opaque => "checker gave up: an unexpanded macro call keeps every caller-derived parameter unknown in this file".to_string(),
                        sigs::DeriveDecline::NoFixpoint => format!(
                            "checker gave up: the caller-derived parameters did not settle in {} rounds — every parameter is read as unknown in this file",
                            sigs::MAX_DERIVE_ROUNDS
                        ),
                    },
                ));
        }
        let mut moved_now: HashSet<Symbol> = derived
            .keys()
            .chain(previous.keys())
            .copied()
            .filter(|name| derived.get(name) != previous.get(name))
            .collect();
        ctx.set_derived_params(derived.clone());
        let mut returns_moved = false;
        // In DEFINITION order, never the map's: a return re-read here is applied at once,
        // so a later name reads an earlier one's new value and the widening is
        // history-dependent — hash order made the verdict nondeterministic (KI-158).
        for &(name, rhs) in all_candidates {
            let Some(Some(base)) = original.get(&name) else {
                continue;
            };
            // Only a DERIVED candidate's return is re-read: an escaped or site-less one
            // keeps its demand-based one, whose `any` lets the recursive call-site
            // specialization sharpen it.
            if ctx.derived_params(name).is_none() {
                continue;
            }
            // A return this round cannot type stays where it is (⊥, or the last round's):
            // the demand-based return would put a value ABOVE the least fixpoint into a
            // monotone ascent, and nothing could bring it back down.
            let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), ctx) else {
                continue;
            };
            typed.insert(name);
            let current = ctx.inferred_fn_sig(name).map(|s| s.ret);
            let ret = ascend_return(ret, current.as_ref(), older_returns.get(&name), round);
            if let Some(current) = current.clone() {
                older_returns.insert(name, current);
            }
            if current != Some(ret.clone()) {
                let mut sig = base.clone();
                sig.ret = ret;
                ctx.add_inferred_fn_sig(name, sig);
                returns_moved = true;
                moved_now.insert(name);
            }
        }
        if derived == previous && !returns_moved {
            // A live function whose return never typed is still at the floor, and the
            // others have been reading it as ⊥. Lift it to its demand-based answer (sound,
            // above ⊥, so the ascent stays monotone) and go again; only a round with
            // nothing left to lift is the fixpoint.
            let mut lifted = false;
            for (&name, sig) in &original {
                if let Some(sig) = sig
                    && at_floor(ctx, &typed, name, sig)
                {
                    ctx.add_inferred_fn_sig(name, sig.clone());
                    lifted = true;
                    moved_now.insert(name);
                }
            }
            if !lifted {
                converged = true;
                break;
            }
            inputs_moved = true;
        } else {
            inputs_moved = returns_moved;
        }
        // What the next derivation must re-walk: everything that moved since the cache's
        // last walk — this round's movers, plus the earlier ones when this round skipped
        // the derivation.
        moved = Some(match moved.take() {
            Some(mut earlier) if !derivation_ran => {
                earlier.extend(moved_now);
                earlier
            }
            _ => moved_now,
        });
        previous = derived;
    }
    timing.site_walks = site_cache.walks;
    timing.site_reuses = site_cache.reuses;
    if !converged {
        for (&name, sig) in &original {
            if let Some(sig) = sig {
                ctx.add_inferred_fn_sig(name, sig.clone());
            }
        }
        ctx.set_derived_params(HashMap::new());
        // Sound — every return is back at its demand-based one and no parameter is derived
        // — and reported: otherwise a file whose derivation gave up reads as clean.
        out.push((
            None,
            format!("checker gave up: the caller-derived parameters and returns did not settle in {MAX_JOINT_ROUNDS} joint rounds — every parameter is read as unknown in this file"),
        ));
        return;
    }
    // A candidate that was not derived, or whose return never typed (still at the floor):
    // give it its demand-based answer back.
    for (&name, sig) in &original {
        if let Some(sig) = sig
            && (ctx.derived_params(name).is_none() || at_floor(ctx, &typed, name, sig))
        {
            ctx.add_inferred_fn_sig(name, sig.clone());
        }
    }
    // …and the candidates that were NOT derived (no site, or an escape), whose returns were
    // read before any return was sharpened, are read again over the sharper ones.
    let undecided: Vec<(Symbol, Value)> = all_candidates
        .iter()
        .filter(|(name, _)| ctx.derived_params(*name).is_none())
        .copied()
        .collect();
    refresh_returns(heap, &undecided, ctx);
}

/// The declaration and definition conformance passes: protocol and behaviour impls,
/// ability call sites and their `:requires` and op collisions, multimethod calls with no
/// method (ADR-179), and each ability impl's return against the op's declared `:-> RET`.
fn check_conformance(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ability_info: &protocol::AbilityInfo,
    multi_info: &protocol::MultiInfo,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let protocols = protocol::collect(heap, forms);
    protocol::check_impls(heap, forms, &protocols, out);
    protocol::check_behaviours(heap, forms, expanded, &protocols, out);
    protocol::check_dispatch_calls(heap, expanded, ability_info, multi_info, out);
    protocol::check_sealed(ability_info, out);
    protocol::check_requires(ability_info, out);
    // Two abilities declaring the same op name would clobber each other's generic
    // function (ADR-172).
    protocol::check_op_collisions(ability_info, out);
    walk::check_impl_returns(heap, expanded, ctx, out);
}

/// The lints that read a form's shape rather than its types, after the body walk: non-tail
/// self-recursion, sealed-`match` exhaustiveness (ADR-187), effects in a `:when` guard, a
/// `catch` that discards its error, a load-time `def` of another module's global, and
/// declared `:pure`/`:total` properties (ADR-351).
/// The `match`, guard and `catch` lints read the UN-expanded forms — those shapes survive
/// only before expansion — and position each warning at the form the reader positioned.
fn run_shape_lints(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    for &form in expanded {
        recursion::check_recursion(heap, form, out);
    }
    exhaustive::check_matches(heap, forms, ctx, out);
    guard_effects::check_guards(heap, forms, out);
    discarded_catch::check_discarded_catches(heap, forms, out);
    let own: Vec<String> = crate::eval::macros::file_modules(heap, forms)
        .into_iter()
        .map(value::symbol_name)
        .collect();
    cross_module_def::check_cross_module_defs(heap, expanded, &own, out);
    properties::check_properties(heap, expanded, ctx, out);
}
/// Unused `(:use …)` imports: a clause whose module contributed importable names, none of
/// which the file references. The module names come from the UN-expanded header (the
/// clause is gone after expansion). Unused module-private `defn`s are a whole-project
/// check (`project-unused-private-warnings`), since other files reach them qualified.
fn lint_unused_imports(
    heap: &Heap,
    forms: &[Value],
    expanded: &[Value],
    out: &mut Vec<(Option<Pos>, String)>,
) {
    // A file with an unresolved BARE reference has an import table that doesn't cover
    // everything it names, so "module M contributed nothing you reference" is not
    // provable — the unresolved name may be M's — and the advice would be to delete an
    // import the code needs. The lint stands down whenever a bare name was unbound.
    let unresolved_bare = out.iter().any(|(_, message)| {
        message
            .strip_prefix("unbound symbol: ")
            .is_some_and(|rest| !rest.split(' ').next().unwrap_or("").contains('/'))
    });
    let use_modules = extract_use_module_names(heap, forms);
    if use_modules.is_empty() || unresolved_bare {
        return;
    }
    // Both trees: a MACRO head (`defserver` from `gen`) is gone from the expanded forms,
    // so a module imported for its macros would read as unused. Counting a name expansion
    // discards can only SUPPRESS a warning — the safe direction for this lint.
    let mut all_refs = collect_all_syms(heap, expanded);
    all_refs.extend(collect_all_syms(heap, forms));
    // Each module's contributed names: the qualified name AND the local alias the file
    // calls, since a `:use` import is normally referenced unqualified.
    let mut module_names: HashMap<String, Vec<Symbol>> = HashMap::new();
    for (local, qualified) in &heap.imported_pairs() {
        let qualified_name = value::symbol_name(*qualified);
        if let Some(slash) = qualified_name.rfind('/') {
            let names = module_names
                .entry(qualified_name[..slash].to_string())
                .or_default();
            names.push(*local);
            names.push(*qualified);
        }
    }
    // A module reached by a qualified `mod/name` anywhere (including its private names,
    // which aren't imported at all) counts its `:use` as load-bearing.
    let qualified_modules: HashSet<String> = all_refs
        .iter()
        .filter_map(|&symbol| {
            let name = value::symbol_name(symbol);
            name.rfind('/').map(|slash| name[..slash].to_string())
        })
        .collect();
    for module_name in &use_modules {
        // Only a module in the import table is actionable — a failed require, a
        // macro-only or an empty module is silent.
        if let Some(names) = module_names.get(module_name) {
            let used = names.iter().any(|symbol| all_refs.contains(symbol))
                || qualified_modules.contains(module_name);
            if !used {
                out.push((None, format!("unused :use import: {module_name}")));
            }
        }
    }
}

/// The per-file budgets that DECLINE rather than answer: past the specialization fuel
/// every re-typing question answers "unknown", and past the expression depth cap a nest
/// types as unknown. Both are sound, and both are reported, so a file the checker largely
/// gave up on does not read as checked.
fn report_exhausted_budgets(out: &mut Vec<(Option<Pos>, String)>) {
    if sigs::fuel_exhausted() {
        out.push((
            None,
            format!(
                "checker gave up: the specialization budget ({} re-typings) ran out — every later call-site specialization in this file read as unknown",
                sigs::MAX_SPECIAL_FUEL
            ),
        ));
    }
    let depth_hits = infer::expr_ty_depth_hits();
    if depth_hits > 0 {
        out.push((
            None,
            format!(
                "checker gave up: {depth_hits} expression{} nested past the depth cap ({}) read as unknown",
                if depth_hits == 1 { "" } else { "s" },
                infer::MAX_EXPR_TY_DEPTH
            ),
        ));
    }
}

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
        guard: sig.guard,
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
        // A `(check-allow :category …)` around a definition is the author suppressing a
        // lint, not hiding the definition: its `def` is inferred like any other, so its
        // callers read a return rather than `any`. `:generated` stays opaque — the copy the linear-map rewrite keeps
        // is not the author's code (KI-152).
        collect_top_level_def(heap, form, &mut out);
    }
    out
}

fn collect_top_level_def(heap: &Heap, form: Value, out: &mut Vec<Value>) {
    if let Some(wrapped) = lint_allowed_body(heap, form) {
        // `check-allow` wraps its forms in one `do`: each of them is a top-level form.
        for &inner in &wrapped {
            collect_top_level_def(heap, inner, out);
        }
        return;
    }
    // A `defn-` (`(do (def …) (%mark-private …))`) or a definition with typed parameters
    // (`(do (def …) (%register-sig …))`, ADR-420) is the definition beside its marks, not a
    // block that hides it: unwrap it, recursively — a typed `defn-` nests one in the other.
    match walk::do_body(heap, form)
        .filter(|body| marks_private(heap, body) || registers_sig(heap, body))
    {
        Some(body) => {
            for inner in body {
                collect_top_level_def(heap, inner, out);
            }
        }
        None => out.push(form),
    }
}

/// Does this `do` body carry a `(%register-sig …)` call beside a `def` — the expansion of a
/// definition with typed parameters (ADR-420)?
fn registers_sig(heap: &Heap, body: &[Value]) -> bool {
    let head_is = |form: Value, name: &str| {
        list_items(heap, form)
            .and_then(|items| items.first().copied())
            .is_some_and(|head| matches!(head, Value::Sym(s) if value::symbol_is(s, name)))
    };
    body.iter().any(|&f| head_is(f, "%register-sig")) && body.iter().any(|&f| head_is(f, "def"))
}

/// The forms a `(%lint-allow :category (do FORM…))` wraps, for every category but
/// `:generated`; `None` for anything else.
fn lint_allowed_body(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    let items = list_items(heap, form)?;
    let [Value::Sym(head), Value::Keyword(category), wrapped] = items.as_slice() else {
        return None;
    };
    if !value::symbol_is(*head, "%lint-allow") || value::symbol_is(*category, "generated") {
        return None;
    }
    Some(walk::do_body(heap, *wrapped).unwrap_or_else(|| vec![*wrapped]))
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

/// A `(def NAME RHS)` form → `(NAME, RHS)`. `None` for anything else — a non-`def` head,
/// a `(defmacro …)` (a special form, never expanded to `def`), or a malformed def.
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
/// image" — the prelude's ability dispatch reads `project/*name*` / `project/*ns-package*` that
/// way, since those ambients are `defdyn`'d by `std/tool/project.blsp` and simply absent
/// under a bare `brood script.blsp`. Flagging the guarded reference as unbound reports a
/// *correct* program, which is exactly what the advisory contract forbids
/// (ADR-123..126) — so the guarded name is exempt for the top-level form that guards it.
///
/// Scoped to one top-level form (usually one `defn`) rather than the whole file so a
/// `bound?` probe in one function can't silence a genuine typo in another.
fn collect_bound_guards(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        if let Some(&Value::Sym(head)) = items.first()
            && value::symbol_is(head, "bound?")
        {
            // `(bound? 'name)` reads as `(bound? (quote name))` in the expanded tree.
            if let Some(quoted) = items.get(1).copied().and_then(|q| list_items(heap, q))
                && matches!(quoted.first(), Some(&Value::Sym(q)) if value::symbol_is(q, kw::QUOTE))
                && let Some(&Value::Sym(name)) = quoted.get(1)
            {
                out.insert(name);
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
/// value-type inference (Gap A) and the same-file return inference (same-file inference) both use this
/// to leave such a global `dynamic()` rather than pinning it to a stale initial value (the
/// lazy-init `(when (nil? *g*) (def *g* (table)))` pattern — else a function returning `*g*`
/// infers `nil` and false-flags its callers). Skips quoted subtrees.
fn count_defs(heap: &Heap, form: Value, counts: &mut HashMap<Symbol, usize>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&Value::Sym(head)) = items.first() else {
            return;
        };
        if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
            return;
        }
        if value::symbol_is(head, kw::DEF)
            && let Some(&Value::Sym(name)) = items.get(1)
        {
            *counts.entry(name).or_insert(0) += 1;
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

/// The same-file return fixpoint over `candidates`, run again: each function's return is
/// re-read from its form, leaf-up, until a pass records nothing new (a cap bounds a
/// pathological chain — the tail stays as it was, which is sound). The caller-derived
/// parameter pass calls it for the candidates it did not derive, once the returns beneath
/// them have been sharpened.
fn refresh_returns(heap: &Heap, candidates: &[(Symbol, Value)], ctx: &mut Ctx) {
    for _ in 0..MAX_RETURN_ROUNDS {
        let mut changed = false;
        for &(name, rhs) in candidates {
            if let Some(ret) = sigs::infer_return_from_form(heap, rhs, Some(name), ctx)
                && ctx.inferred_fn_sig(name).map(|s| s.ret) != Some(ret.clone())
            {
                let sig = match sigs::infer_params_from_form(heap, rhs, ctx) {
                    Some(demands) => demands.into_sig(ret),
                    None => crate::types::Sig::new(Vec::new(), ret),
                };
                ctx.add_inferred_fn_sig(name, sig);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}
