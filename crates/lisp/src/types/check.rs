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
mod guards;
mod hygiene;
mod protocol;
mod recursion;
mod sigs;
mod walk;

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::Value;
use crate::error::Pos;

use ctx::Ctx;
use walk::{check_into, collect_def_names};

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
        // matches what `eval` will see (qualified defs + references).
        let exp = crate::eval::macros::compile(heap, f, root).unwrap_or(f);
        // Root the just-built expansion *before* possibly triggering a
        // collect via `eval`; otherwise this LOCAL handle dies between
        // here and the next iteration's macroexpand.
        heap.push_root(exp);
        expanded.push(exp);
        // Evaluate `(require …)` (so a module's macros/globals resolve) and the
        // `(ns …)`/`(defmodule …)` header (so its `(:use …)` imports populate the
        // import table). `f` is the un-expanded form — its head still names the
        // header before macroexpansion lowered it to a `do`.
        if is_require_form(heap, exp) || is_ns_header(heap, f) {
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
    let mut known_ns = std::collections::HashSet::new();
    for sym in heap.global_symbols() {
        let name = crate::core::value::symbol_name(sym);
        if let Some(slash) = name.rfind('/') {
            known_ns.insert(name[..=slash].to_string());
        }
    }
    ctx.set_known_ns(known_ns);
    for &form in &expanded {
        collect_def_names(heap, form, &mut ctx);
    }
    // Pass 2.5: collect `(sig name (… -> …))` declarations from the *un-expanded*
    // forms (the `sig` macro expands to nil, so the declaration is gone in
    // `expanded` — same reason the hygiene lint reads un-expanded forms). These
    // become the authoritative signatures the call-check consults first.
    for &form in &forms {
        if let Some((name, sig)) = annot::parse_sig_decl(heap, form) {
            ctx.add_declared_sig(name, sig);
        }
        if let Some((name, sv)) = annot::parse_sig_decl_with_vars(heap, form) {
            ctx.add_declared_sig_with_vars(name, sv);
        }
        // Non-arrow `(sig x T)` value-type declarations — consumed by the
        // gradual-assignment check on `(def x …)` (the first `GradualTy` consumer).
        if let Some((name, ty)) = annot::parse_value_sig_decl(heap, form) {
            ctx.add_declared_value_ty(name, ty);
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
    // Balance the GC roots we pushed for pass 1 (input forms + their
    // expansions). Safe to drop now: nothing after this consults `expanded`
    // or `forms` against the heap.
    heap.truncate_roots(roots_base);
    heap.set_compile_ns(prev_ns);
    heap.set_ns_known_names(prev_known);
    heap.set_imports(prev_imports);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // The submodules' items are still accessed by name in these tests —
    // import them explicitly now that they're not all in this file.
    use super::sigs::primitive_sig;
    use crate::core::value::Tag;
    use crate::syntax::reader;
    use crate::types::Ty;

    /// A full `Interp` — primitives + the loaded prelude. We need the prelude
    /// in the global env so the new unbound-symbol diagnostic doesn't false-
    /// flag every Brood-side stdlib name (`list`, `int?`, `zero?`, `inc`, …);
    /// the previous primitives-only setup worked when the checker silently
    /// skipped unknown callees, but Step 4's unbound check has to know what's
    /// genuinely bound.
    fn warnings(src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        check_form(&interp.heap, form)
    }

    /// `warnings` but with macroexpansion — what `(check 'form)` and
    /// `check-file` actually do. Required to exercise post-expansion shapes
    /// like `match` (a `defmacro` whose pattern compiler lowers to
    /// `let`+`if`+`%eq`), threading macros, and the test-framework wrappers.
    fn warnings_expanded(src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        let form =
            crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
        check_form(&interp.heap, form)
    }

    /// Whole-file checking — what `nest check` runs. Unlike [`warnings`] (a bare
    /// fragment), this enables operand / value-slot unbound checking and threads
    /// file-local def names, so it exercises the strict, file-mode behaviour.
    fn file_warnings(src: &str) -> Vec<String> {
        let interp = crate::Interp::new();
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
        check_file(&mut heap, &forms)
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    }

    // ---- protocol conformance (Pass 2.6) ----
    // `file_warnings` returns *all* diagnostics; these assert on the protocol ones
    // with `.contains` (the un-defined `defprotocol`/`defimpl` macros also draw
    // unbound-symbol noise in a bare test interp, which is irrelevant here).

    #[test]
    fn protocol_flags_a_missing_op() {
        let ws = file_warnings("(defprotocol P (a [x]) (b [x]))\n(defimpl P :int (a [x] x))");
        assert!(ws.iter().any(|w| w.contains("missing op `b`")), "{ws:?}");
    }

    #[test]
    fn protocol_flags_an_arity_mismatch() {
        let ws = file_warnings("(defprotocol P (a [x]))\n(defimpl P :int (a [x y] x))");
        assert!(
            ws.iter().any(|w| w.contains("`a` takes 1 arg(s), this impl has 2")),
            "{ws:?}"
        );
    }

    #[test]
    fn protocol_flags_an_undeclared_method() {
        let ws = file_warnings("(defprotocol P (a [x]))\n(defimpl P :int (a [x] x) (z [x] x))");
        assert!(ws.iter().any(|w| w.contains("has no op `z`")), "{ws:?}");
    }

    #[test]
    fn protocol_complete_impl_is_clean() {
        let ws = file_warnings("(defprotocol P (a [x]) (b [x]))\n(defimpl P :int (a [x] x) (b [x] x))");
        assert!(!ws.iter().any(|w| w.contains("missing op")), "{ws:?}");
        assert!(!ws.iter().any(|w| w.contains("has no op")), "{ws:?}");
        assert!(!ws.iter().any(|w| w.contains("takes")), "{ws:?}");
    }

    // ---- behaviour conformance: `(:implements …)` on a module ----

    #[test]
    fn behaviour_flags_a_missing_callback() {
        let ws = file_warnings(
            "(defbehaviour B (render [m]) (mount [p]))\n(defmodule foo (:implements B))\n(defn render (m) m)",
        );
        assert!(ws.iter().any(|w| w.contains("behaviour B: this module is missing `mount`")), "{ws:?}");
    }

    #[test]
    fn behaviour_flags_an_arity_mismatch() {
        let ws = file_warnings(
            "(defbehaviour B (render [m]))\n(defmodule foo (:implements B))\n(defn render (m extra) m)",
        );
        assert!(ws.iter().any(|w| w.contains("`render` takes 2 arg(s), the behaviour needs 1")), "{ws:?}");
    }

    #[test]
    fn behaviour_complete_module_is_clean() {
        let ws = file_warnings(
            "(defbehaviour B (render [m]))\n(defmodule foo (:implements B))\n(defn render (m) m)",
        );
        // No conformance diagnostic (the bare-interp "unbound symbol: defbehaviour"
        // noise contains the substring "behaviour", so match the real messages).
        assert!(
            !ws.iter().any(|w| w.contains("module is missing") || w.contains("the behaviour needs")),
            "{ws:?}"
        );
    }

    /// The non-tail-recursion lint (`recursion::check_recursion`) over a
    /// macroexpanded form — what `check-file`'s Pass 3.5 runs.
    fn recursion_warnings(src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        let form =
            crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
        let mut out = Vec::new();
        recursion::check_recursion(&interp.heap, form, &mut out);
        out.into_iter().map(|(_, m)| m).collect()
    }

    #[test]
    fn flags_non_tail_self_recursion() {
        // self-call as an argument to another call
        assert!(
            recursion_warnings("(defn fact (n) (if (= n 0) 1 (* n (fact (- n 1)))))")
                .iter()
                .any(|w| w.contains("fact") && w.contains("non-tail"))
        );
        assert!(recursion_warnings(
            "(defn sum (xs) (if (empty? xs) 0 (+ (first xs) (sum (rest xs)))))"
        )
        .iter()
        .any(|w| w.contains("sum")));
        // self-call as a let binding value
        assert!(!recursion_warnings("(defn k (n) (let (m (k (- n 1))) m))").is_empty());
        // first (tested) operand of `and`, and a `cond` test
        assert!(!recursion_warnings("(defn p (n) (and (p n) (> n 0)))").is_empty());
        assert!(!recursion_warnings("(defn g (n) (cond (g 0) :a :else :b))").is_empty());
    }

    #[test]
    fn no_warning_for_tail_recursion_or_higher_order() {
        // proper tail calls in each tail-propagating special form
        assert!(recursion_warnings(
            "(defn loop (n acc) (if (= n 0) acc (loop (- n 1) (* acc n))))"
        )
        .is_empty());
        assert!(recursion_warnings("(defn down (n) (when (> n 0) (down (- n 1))))").is_empty());
        assert!(recursion_warnings("(defn f (n) (cond (= n 0) :z :else (f (- n 1))))").is_empty());
        assert!(recursion_warnings("(defn p (n) (and (> n 0) (p (- n 1))))").is_empty());
        assert!(recursion_warnings("(defn k (n) (let (m (- n 1)) (k m)))").is_empty());
        // a self-call inside a nested closure is a different frame — not flagged
        assert!(recursion_warnings("(defn h (xs) (map (fn (x) (h x)) xs))").is_empty());
        // non-recursive function
        assert!(recursion_warnings("(defn g (x) (+ x 1))").is_empty());
    }

    #[test]
    fn flags_literal_misuse_of_primitives() {
        assert!(warnings("(first 5)")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
        // A keyword literal now infers as its singleton type, so the diagnostic
        // names the exact value (`:k`) rather than the coarse `keyword` tag.
        assert!(warnings("(string-length :k)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains(":k")));
        assert!(warnings("(%add 1 \"x\")")
            .iter()
            .any(|w| w.contains("%add")));
        assert!(warnings("(vector-ref [1 2] :k)")
            .iter()
            .any(|w| w.contains("vector-ref")));
    }

    #[test]
    fn no_false_positives_when_type_is_unknown_or_right() {
        assert!(warnings("(first (list 1 2))").is_empty()); // arg is a non-sig call → dynamic
        assert!(warnings("(first xs)").is_empty()); // variable → dynamic
        assert!(warnings("(first [1 2 3])").is_empty()); // vector is allowed
        assert!(warnings("(%add 1 2)").is_empty());
        assert!(warnings("(string-length \"hi\")").is_empty());
    }

    #[test]
    fn propagates_primitive_result_types() {
        // string-length returns int; first wants a list/vector → flag the int.
        assert!(warnings("(first (string-length \"a\"))")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
    }

    #[test]
    fn an_any_result_is_not_a_false_positive() {
        // vector-ref's result type is `any` (unknown), so feeding it to
        // string-length (wants string) must NOT warn — `any` overlaps `string`.
        assert!(warnings("(string-length (vector-ref [1] 0))").is_empty());
    }

    #[test]
    fn does_not_descend_into_quote() {
        assert!(warnings("(quote (first 5))").is_empty());
    }

    #[test]
    fn curated_closures_are_checked() {
        // `+`, `<`, `map` are Brood closures, but their curated sigs let us flag
        // provable misuse — the headline cases.
        assert!(warnings("(+ 1 \"x\")")
            .iter()
            .any(|w| w.contains('+') && w.contains("number")));
        assert!(warnings("(< 1 :k)").iter().any(|w| w.contains('<')));
        // map's first argument must be callable; an int is not.
        assert!(warnings("(map 1 xs)")
            .iter()
            .any(|w| w.contains("map") && w.contains("argument 1")));
        // Correct uses, and an unknown (variable) callable, stay silent.
        assert!(warnings("(+ 1 2)").is_empty());
        assert!(warnings("(map inc xs)").is_empty()); // inc is a variable → unknown
    }

    #[test]
    fn sig_declaration_is_read_by_the_checker() {
        // A user (sig …) gives a branchy fn a signature the checker trusts:
        // arguments checked against the declared params.
        let w = file_warnings("(sig f (int -> int))\n(defn f (x) (if (> x 0) x (- x)))\n(f \"s\")");
        assert!(
            w.iter()
                .any(|m| m.contains("f:") && m.contains("argument 1") && m.contains("int")),
            "declared param type should flag (f \"s\"): {w:?}"
        );
        // The declared *result* flows out: f : int, string-length wants string.
        let w = file_warnings("(sig f (int -> int))\n(defn f (x) x)\n(string-length (f 3))");
        assert!(
            w.iter().any(|m| m.contains("string-length")),
            "declared result type should flag string-length: {w:?}"
        );
        // Correct uses stay silent.
        let w = file_warnings("(sig f (int -> int))\n(defn f (x) x)\n(f 3)\n(+ 1 (f 4))");
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "correct uses of a declared fn must be silent: {w:?}"
        );
    }

    #[test]
    fn keyword_literal_types_in_a_sig_are_enforced() {
        // A parameter typed as an enumerated keyword set flags a keyword outside it.
        let w = file_warnings("(sig f ((or :a :b) -> int))\n(defn f (x) 1)\n(f :c)");
        assert!(
            w.iter()
                .any(|m| m.contains("f:") && m.contains("argument 1") && m.contains(":a | :b")),
            "a keyword outside the literal set should flag, naming it: {w:?}"
        );
        // A member of the set is fine.
        let w = file_warnings("(sig f ((or :a :b) -> int))\n(defn f (x) 1)\n(f :a)");
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a keyword in the set must be silent: {w:?}"
        );
        // The declared literal *result* flows out and is checked too.
        let w = file_warnings(
            "(sig mode (-> (or :maximized :fullscreen)))\n(defn mode () :maximized)\n(string-length (mode))",
        );
        assert!(
            w.iter().any(|m| m.contains("string-length")),
            "a keyword-literal result feeding string-length should flag: {w:?}"
        );
    }

    #[test]
    fn sig_declaration_handles_arity_unions_and_bad_exprs() {
        // Arity comes from the declared param count for a file-local defn the
        // read-only checker can't otherwise inspect.
        let w = file_warnings("(sig g (int int -> int))\n(defn g (a b) (+ a b))\n(g 1)");
        assert!(
            w.iter().any(|m| m.contains("expected 2")),
            "declared arity should flag (g 1): {w:?}"
        );
        // Union result type: (or int nil) — feeding it to a sink that wants a
        // string is still a provable mismatch.
        let w =
            file_warnings("(sig h (int -> (or int nil)))\n(defn h (x) x)\n(string-length (h 1))");
        assert!(
            w.iter().any(|m| m.contains("string-length")),
            "union result (int|nil) is disjoint from string: {w:?}"
        );
        // An unparseable type-expr is dropped — never a false signal.
        let w = file_warnings("(sig k (bogus -> int))\n(defn k (x) x)\n(k \"s\")");
        assert!(
            w.iter()
                .all(|m| !m.contains("k:") || !m.contains("argument")),
            "an unrecognised type-expr must be ignored, not guessed: {w:?}"
        );
    }

    #[test]
    fn variadic_defn_with_sig_does_not_get_a_false_arity_warning() {
        // Regression: the `(sig …)` parser only builds *fixed*-arity sigs, so a
        // sig on a **variadic** defn would record an exact arity equal to the
        // declared param count. A read-only whole-file check can't inspect the
        // real (unevaluated) closure, so it falls back to that count — and a call
        // with more args than the sig lists would falsely warn. The def site's
        // own `& rest` must suppress the sig-derived exact arity.
        let w = file_warnings("(sig f (int -> int))\n(defn f (x & rest) x)\n(f 1 2 3)");
        assert!(
            w.iter()
                .all(|m| !(m.contains("f:") && m.contains("number of arguments"))),
            "a variadic defn must not get a false arity warning: {w:?}"
        );
        // `&rest` spelling, and below the declared count is fine too.
        let w = file_warnings("(sig g (int int -> int))\n(defn g (a &rest more) a)\n(g 1 2 3 4)");
        assert!(
            w.iter()
                .all(|m| !(m.contains("g:") && m.contains("number of arguments"))),
            "&rest variadic defn must not get a false arity warning: {w:?}"
        );
        // A multi-arity fn with a variadic arm is likewise variadic.
        let w = file_warnings("(sig h (int -> int))\n(defn h ((x) x) ((x & ys) x))\n(h 1 2 3)");
        assert!(
            w.iter()
                .all(|m| !(m.contains("h:") && m.contains("number of arguments"))),
            "multi-arity variadic defn must not get a false arity warning: {w:?}"
        );
        // Control: a *fixed*-arity sig'd defn STILL gets its arity checked (the
        // fix must not over-suppress) — mirrors the case above.
        let w = file_warnings("(sig p (int int -> int))\n(defn p (a b) (+ a b))\n(p 1)");
        assert!(
            w.iter().any(|m| m.contains("expected 2")),
            "fixed-arity sig'd defn must still be arity-checked: {w:?}"
        );
    }

    #[test]
    fn dead_clause_flagged_for_a_sig_typed_param() {
        // A `match` literal pattern that can't match the parameter's declared type.
        let w = file_warnings(
            "(sig f (int -> keyword))\n(defn f (n) (match n (\"hi\" :s) (_ :other)))",
        );
        assert!(
            w.iter()
                .any(|m| m.contains("unreachable clause") && m.contains("int")),
            "a string-literal clause when n : int should be dead: {w:?}"
        );
        // A `cond` predicate disjoint from the declared parameter type.
        let w =
            file_warnings("(sig g (int -> keyword))\n(defn g (n) (cond (string? n) :s :else :o))");
        assert!(
            w.iter().any(|m| m.contains("unreachable clause")),
            "(string? n) when n : int should be dead: {w:?}"
        );
    }

    #[test]
    fn dead_clause_silent_without_sig_or_when_compatible_or_a_literal_scrutinee() {
        // No `sig` → the parameter is untyped → never flagged (no false positive).
        assert!(
            file_warnings("(defn k (n) (match n (\"hi\" :s) (_ :o)))")
                .iter()
                .all(|m| !m.contains("unreachable")),
            "no sig ⇒ no dead-clause"
        );
        // A recognised but *compatible* guard narrows, it isn't dead.
        assert!(
            file_warnings("(sig h (int -> keyword))\n(defn h (n) (cond (int? n) :i :else :o))")
                .iter()
                .all(|m| !m.contains("unreachable")),
            "(int? n) when n : int must not flag"
        );
        // A literal scrutinee is not a sig-typed param — the gate excludes it (this
        // is the intentional non-match test shape that the naive lint flagged).
        assert!(
            file_warnings("(defn m () (match [1 2] ((a) :one) (_ :o)))")
                .iter()
                .all(|m| !m.contains("unreachable")),
            "a literal scrutinee must never be flagged dead"
        );
    }

    #[test]
    fn curated_helper_sigs_catch_misuse() {
        // even?/odd?/abs require a number.
        assert!(warnings("(even? \"x\")")
            .iter()
            .any(|w| w.contains("even?") && w.contains("number")));
        assert!(warnings("(odd? :k)")
            .iter()
            .any(|w| w.contains("odd?") && w.contains("number")));
        assert!(warnings("(abs :k)")
            .iter()
            .any(|w| w.contains("abs") && w.contains("number")));
        // count/length want a string | map | sequence, not a number.
        assert!(warnings("(count 5)").iter().any(|w| w.contains("count")));
        assert!(warnings("(length :k)").iter().any(|w| w.contains("length")));
        // not/zero? accept any arg but pin a bool *result*, so feeding it to a
        // numeric sink is caught (the result-type payoff).
        assert!(warnings("(+ 1 (not x))")
            .iter()
            .any(|w| w.contains('+') && w.contains("bool")));
        assert!(warnings("(+ 1 (zero? x))")
            .iter()
            .any(|w| w.contains('+') && w.contains("bool")));
        // Correct uses stay silent (no false positives).
        for ok in [
            "(even? 4)",
            "(abs -3)",
            "(count [1 2 3])",
            "(count \"hi\")",
            "(not x)",
            "(zero? n)",
        ] {
            assert!(
                warnings(ok).iter().all(|w| !w.contains("expects")),
                "{ok} should be silent: {:?}",
                warnings(ok)
            );
        }
    }

    #[test]
    fn curated_output_and_numeric_sigs() {
        // println/eprintln/eprint return nil — feeding to a numeric sink is caught.
        for f in ["println", "eprintln", "eprint"] {
            let w = warnings(&format!("(+ 1 ({f} \"hi\"))"));
            assert!(
                w.iter().any(|s| s.contains('+') && s.contains("nil")),
                "{f}: expected '+' nil-result warning, got {w:?}"
            );
        }
        // min/max require at least one number.
        assert!(warnings("(min \"a\" 2)")
            .iter()
            .any(|w| w.contains("min") && w.contains("number")));
        assert!(warnings("(max 1 :k)")
            .iter()
            .any(|w| w.contains("max") && w.contains("number")));
        // min/max return a number — feeding to a string sink is caught.
        assert!(warnings("(string-length (min 1 2))")
            .iter()
            .any(|w| w.contains("string-length")));
        // Correct uses stay silent.
        for ok in [
            "(println \"hi\")",
            "(min 1 2 3)",
            "(max 0.5 1.5)",
            "(+ 1 (min 2 3))",
        ] {
            assert!(
                warnings(ok).iter().all(|w| !w.contains("expects")),
                "{ok} should be silent: {:?}",
                warnings(ok)
            );
        }
    }

    #[test]
    fn skips_error_testing_forms() {
        // `try` and the error-asserting helpers deliberately exercise failures,
        // so misuse inside them is not flagged.
        assert!(warnings("(try (first 5) (catch e e))").is_empty());
        assert!(warnings("(error-of (first 5))").is_empty());
        assert!(warnings("(assert-error (first 5))").is_empty());
        // ...but a sibling form outside the skipped one is still checked.
        assert!(!warnings("(do (first 5) (try (first 6) (catch e e)))").is_empty());
    }

    #[test]
    fn map_kv_refinement_flows_through_checker() {
        // (sig f ((map keyword int) -> int)): the get result is int | nil.
        // Feeding that to string-length should warn. Without the sig the result
        // type is unknown → no warning, so the sig must be declared — use
        // file_warnings so the `sig` form is parsed.
        let src = "
(defn f (m) (get m :k))
(sig f ((map keyword int) -> int))
(string-length (f {:a 1}))
";
        let w = file_warnings(src);
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "expected string-length warning for int|nil arg, got {w:?}"
        );

        // `(keys m)` where m : map<keyword, int> → nil | list<keyword>.
        // Feeding to string-length warns (list is not a string).
        let src2 = "
(defn g (m) (keys m))
(sig g ((map keyword int) -> (list keyword)))
(string-length (g {:a 1}))
";
        let w2 = file_warnings(src2);
        assert!(
            w2.iter().any(|s| s.contains("string-length")),
            "expected string-length warning for list<keyword> arg, got {w2:?}"
        );

        // Correct uses stay silent.
        for ok in [
            "(get {:a 1} :a)", // any map get — flat result, no warning
            "(keys {:a 1})",
            "(vals {:a 1})",
        ] {
            assert!(
                warnings(ok).iter().all(|w| !w.contains("expects")),
                "{ok} should be silent: {:?}",
                warnings(ok)
            );
        }
    }

    #[test]
    fn covers_the_other_signed_primitives() {
        assert!(warnings("(mod 7 3)").is_empty());
        assert!(warnings("(mod 7 \"x\")").iter().any(|w| w.contains("mod")));
        assert!(warnings("(rem :a 3)").iter().any(|w| w.contains("rem")));
        assert!(warnings("(vector-length 5)")
            .iter()
            .any(|w| w.contains("vector-length")));
        assert!(warnings("(substring \"hi\" \"a\" 1)")
            .iter()
            .any(|w| w.contains("substring") && w.contains("argument 2")));
        assert!(warnings("(%lt 1 :k)").iter().any(|w| w.contains("%lt")));
    }

    #[test]
    fn reports_each_bad_argument() {
        // Both args provably wrong → two distinct warnings (one per position).
        let w = warnings("(mod \"a\" :b)");
        assert_eq!(w.len(), 2, "{:?}", w);
        assert!(w.iter().any(|s| s.contains("argument 1")));
        assert!(w.iter().any(|s| s.contains("argument 2")));
    }

    #[test]
    fn nested_misuse_is_found() {
        // A wrong call buried inside an argument is still reported.
        let w = warnings("(vector-length (cons (first 5) 2))");
        assert!(w.iter().any(|s| s.contains("first")));
    }

    #[test]
    fn atoms_and_malformed_forms_do_not_panic() {
        for src in ["5", "foo", "\"s\"", ":k", "()", "(5 6 7)", "(first)"] {
            // No panic, and no spurious warning on a bare atom / non-symbol head /
            // missing argument.
            let _ = warnings(src);
        }
        assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol — no diagnostics
                                                 // `(first)` is now an arity diagnostic (0 args; first needs 1).
        assert!(warnings("(first)")
            .iter()
            .any(|w| w.contains("first") && w.contains("expected 1")));
    }

    // ------------- Step 3: sigs sourced from NativeFn, closure inference --------------

    /// The eight test cases below need real user-defined closures, which means
    /// running a `defn` against the global table. The `Interp` builds the full
    /// prelude (curated stdlib closures and all) on top of the primitive kernel
    /// — exactly the surface a checker is supposed to see.
    fn check_with_defs(defs: &[&str], src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        for d in defs {
            interp.eval_str(d).expect("def");
        }
        let form =
            crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse expression");
        // Macro-expand so any prelude wrappers (defn → fn, etc.) are gone, like
        // `brood --check`/the `check` builtin do before calling check_form.
        let form =
            crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
        check_form(&interp.heap, form)
    }

    #[test]
    fn primitive_sigs_are_read_from_native_fn() {
        // The point of Step 3: there is no parallel `primitive_sig` table.
        // The sig the checker uses for `string-length` *is* the one declared
        // next to its `Arity` in `builtins.rs`. If we ever drop the sig field
        // (or set it wrong), this catches it.
        let interp = crate::Interp::new();
        let sig = primitive_sig(&interp.heap, crate::core::value::intern("string-length"))
            .expect("string-length is a primitive");
        assert_eq!(sig.params, vec![Ty::of(Tag::Str)]);
        assert_eq!(sig.ret, Ty::of(Tag::Int));
        // The "no useful info" lane: a variadic any-arg primitive (str) returns
        // a Sig that param-overlaps every input, so it never warns.
        let any_sig = primitive_sig(&interp.heap, crate::core::value::intern("str"))
            .expect("str is a primitive");
        assert_eq!(any_sig.rest, Some(Ty::ANY));
    }

    #[test]
    fn infers_a_straight_line_wrapper() {
        // (defn inc (x) (+ x 1)) → x : number (from +'s rest type).
        // So `(inc :k)` is a provable misuse.
        let w = check_with_defs(&["(defn inc (x) (+ x 1))"], "(inc :k)");
        assert!(
            w.iter().any(|s| s.contains("inc") && s.contains("number")),
            "expected an `inc :k` warning, got {:?}",
            w
        );
    }

    #[test]
    fn inferred_return_type_propagates() {
        // (defn inc (x) (+ x 1)) returns the number `+` returns; feeding it into
        // `string-length` (wants string) is a provable misuse.
        let w = check_with_defs(&["(defn inc (x) (+ x 1))"], "(string-length (inc 1))");
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "expected a `string-length` warning, got {:?}",
            w
        );
    }

    #[test]
    fn inferred_params_intersect_across_positions() {
        // (defn add (x y) (+ x y)) — both x and y at + positions → number.
        let w = check_with_defs(&["(defn add (x y) (+ x y))"], "(add \"a\" 2)");
        assert!(w.iter().any(|s| s.contains("add")), "got {:?}", w);
    }

    #[test]
    fn does_not_infer_through_branches_or_lets() {
        // A body with `if`/complex `let` is *not* a single straight-line expression
        // — inference must skip it, leaving the closure untyped (no warning).
        // (A plain let-alias `(let (y x) call)` IS inferred — see below.)
        let w = check_with_defs(&["(defn maybe (x) (if (int? x) (+ x 1) x))"], "(maybe :k)");
        assert!(
            w.is_empty(),
            "if-branching bodies must not infer (so no warning): {:?}",
            w
        );
    }

    #[test]
    fn infers_through_let_alias() {
        // `(let (y x) call)` where y is just a rename of closure param x:
        // the body is still one straight-line call — inference should work.
        let w = check_with_defs(
            &["(defn double (x) (let (y x) (* y 2)))"],
            "(string-length (double 3))",
        );
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "let-alias wrapper should not block infer_sig: {:?}",
            w
        );
        // The param type is also inferred: `y` at number position → x : number.
        let w = check_with_defs(&["(defn double (x) (let (y x) (* y 2)))"], "(double :k)");
        assert!(
            w.iter()
                .any(|s| s.contains("double") && s.contains("number")),
            "let-alias: param type should propagate from callee: {:?}",
            w
        );
        // A non-param let (binding a computed value, not a param rename) is NOT peeled.
        let w = check_with_defs(
            &["(defn wrap (x) (let (y (+ x 1)) (* y 2)))"],
            "(string-length (wrap 3))",
        );
        assert!(
            w.is_empty(),
            "let with non-param RHS must not be peeled — no inference: {:?}",
            w
        );
    }

    #[test]
    fn does_not_infer_through_recursion() {
        // A self-recursive call has no fixed sig to read from — must skip,
        // even though the body is structurally a single call.
        let w = check_with_defs(&["(defn loop (x) (loop x))"], "(loop :k)");
        assert!(w.is_empty(), "recursive defns must not infer: {:?}", w);
    }

    #[test]
    fn skips_inference_for_variadic_or_optional_closures() {
        // A variadic-tail closure isn't a "fixed-arity straight-line" — skip.
        let w = check_with_defs(&["(defn vlist (& xs) (first xs))"], "(vlist 1 2 3)");
        assert!(w.is_empty(), "variadic defns must not infer: {:?}", w);
    }

    // ------------- Step 4: scope tracking + guard narrowing --------------

    #[test]
    fn let_binding_propagates_its_rhs_type() {
        // The RHS is a literal int — `(first x)` should flag, because x : int
        // shadows "unknown" in the body. (This is the basic let-tracking.)
        let w = warnings("(let (x 1) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected a `first x` warning where x : int, got {:?}",
            w
        );
    }

    #[test]
    fn let_binding_from_nested_call_propagates() {
        // RHS is a known primitive whose return type is int. So `x : int`,
        // and `(first x)` flags.
        let w = warnings("(let (x (string-length \"hi\")) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected a `first x` warning where x : int, got {:?}",
            w
        );
    }

    #[test]
    fn let_binding_of_unknown_rhs_stays_silent() {
        // RHS is a variable (unknown), so x stays unknown — `(first x)` must
        // not warn. (No false positives from let-tracking.)
        let w = warnings("(let (x foo) (first x))");
        assert!(w.is_empty(), "got {:?}", w);
    }

    #[test]
    fn inner_let_shadows_outer_binding() {
        // The outer x : int; the inner x : string. `(first x)` in the body
        // refers to the inner, which is a string — and `first` accepts list /
        // vector, disjoint from string. So a warning is still expected, but
        // the *narrowing message* must be "string", not "int". This is the
        // shadowing-correctness check (outer narrowing must not leak in).
        let w = warnings("(let (x 1) (let (x \"hi\") (first x)))");
        assert!(
            w.iter()
                .any(|s| s.contains("first") && s.contains("string")),
            "expected the inner string to be the source, got {:?}",
            w
        );
        assert!(
            !w.iter().any(|s| s.contains("got int")),
            "outer int must not leak through shadowing: {:?}",
            w
        );
    }

    #[test]
    fn shadowing_with_unknown_rhs_clears_prior_narrowing() {
        // Outer x : int; inner x : <unknown var>. Inside the inner let, x is
        // unknown — `(first x)` must NOT warn (the outer narrowing must not
        // leak through the shadow).
        let w = warnings("(let (x 1) (let (x foo) (first x)))");
        assert!(w.is_empty(), "shadow must clear the prior type: {:?}", w);
    }

    #[test]
    fn vector_let_bindings_are_recognised() {
        // `(let [x 1] …)` (vector shape) must work the same as `(let (x 1) …)`.
        let w = warnings("(let [x 1] (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "vector-form let bindings must populate the ctx: {:?}",
            w
        );
    }

    #[test]
    fn guard_narrowing_lets_a_then_branch_flag_a_misuse() {
        // In the then-branch of `(if (int? x) …)`, x : int — `(first x)` flags.
        let w = warnings("(if (int? x) (first x) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected guard narrowing to flag (first x) when x : int, got {:?}",
            w
        );
    }

    #[test]
    fn guard_narrowing_does_not_leak_into_the_else_branch() {
        // The else-branch narrows x to `not int`, which overlaps list / vector;
        // so `(first x)` must NOT warn there.
        let w = warnings("(if (int? x) nil (first x))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "else branch must not have x narrowed to int: {:?}",
            w
        );
    }

    #[test]
    fn negated_guard_flips_the_narrowing() {
        // (if (not (int? x)) …) — the then-branch narrows x to `not int`, the
        // else-branch to int.
        let w = warnings("(if (not (int? x)) nil (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "the else of a negated guard must narrow to the inner type: {:?}",
            w
        );
    }

    #[test]
    fn guards_for_number_and_list_unions_narrow_to_the_union() {
        // (if (number? x) (first x) …) — x : number = int|float in the then,
        // which is disjoint from list/vector, so `(first x)` flags.
        let w = warnings("(if (number? x) (first x) nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("first") && s.contains("number")),
            "number? must narrow to int|float: {:?}",
            w
        );
        // The list? guard should *not* warn in the then (list overlaps first's
        // expected type).
        let w = warnings("(if (list? x) (first x) nil)");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "list? must not produce a false positive on (first x): {:?}",
            w
        );
    }

    #[test]
    fn non_guard_tests_dont_narrow() {
        // The test isn't a recognised type predicate, so x stays unknown in
        // both branches — `(first x)` must not warn.
        let w = warnings("(if (zero? x) (first x) (first x))");
        assert!(w.is_empty(), "non-tag-guard test must not narrow: {:?}", w);
    }

    #[test]
    fn nested_guards_compose_their_narrowings() {
        // (if (number? x) (if (int? x) … (first x)) …) — in the inner else,
        // x is narrowed to `number ∩ ¬int` = float, which is still disjoint
        // from list/vector, so `(first x)` flags.
        let w = warnings("(if (number? x) (if (int? x) nil (first x)) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("float")),
            "nested guards must compose to float (= number ∩ ¬int): {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_narrows_when_used_as_an_if_test() {
        // The user-written shape `(let (cond (int? x)) (if cond …))` — Brood is
        // immutable, so `cond` faithfully reflects `(int? x)` until the let
        // ends. The guard-alias table maps `cond → (x, int)`, and the inner
        // `if cond` narrows x to int in the then-branch.
        let w = warnings("(let (cond (int? x)) (if cond (first x) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected let-bound guard to flag (first x) in the then: {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_narrows_in_the_else_branch_too() {
        // Else-branch sees x as `not int`, which overlaps list / vector, so
        // no warning — same as the direct-test case.
        let w = warnings("(let (cond (int? x)) (if cond nil (first x)))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "the else of a let-bound guard must narrow to ¬int, not int: {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_can_be_negated_in_the_if() {
        // `(if (not cond) …)` flips the narrowing — same as `(not (int? x))`.
        let w = warnings("(let (cond (int? x)) (if (not cond) nil (first x)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected negation to flip the let-bound guard: {:?}",
            w
        );
    }

    #[test]
    fn rebinding_the_guard_name_clears_the_alias() {
        // After `(let (cond <unknown>) …)` shadowing, `cond` no longer aliases
        // the int-guard, so `(if cond …)` must not narrow x.
        let w = warnings("(let (cond (int? x)) (let (cond foo) (if cond (first x) nil)))");
        assert!(w.is_empty(), "shadowing must drop the guard alias: {:?}", w);
    }

    #[test]
    fn rebinding_to_a_non_guard_value_clears_the_alias() {
        // Same as above but with an int literal rather than an unknown var.
        let w = warnings("(let (cond (int? x)) (let (cond 1) (if cond (first x) nil)))");
        assert!(
            w.is_empty(),
            "shadowing with a non-guard value must drop the alias: {:?}",
            w
        );
    }

    #[test]
    fn self_aliased_guard_is_not_recorded() {
        // `(let (x (int? x)) …)` shadows the outer x with a bool; the inner
        // body's `x` is the bool, not the original — narrowing the original
        // would be unsound (it's no longer reachable), so we must not record
        // the guard. (No assertion about a warning either way — the point is
        // we don't crash and don't introduce a stale alias.)
        let w = warnings("(let (x (int? x)) (if x x nil))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "self-aliased guards must not propagate to inner uses: {:?}",
            w
        );
    }

    #[test]
    fn let_inside_a_then_branch_can_shadow_a_narrowing() {
        // Outer narrowing: x : int. Inner shadow: x : string. The body now
        // sees x as string, so the narrowing message names string.
        let w = warnings("(if (int? x) (let (x \"hi\") (first x)) nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("first") && s.contains("string")),
            "shadow must override the guard narrowing: {:?}",
            w
        );
        assert!(
            !w.iter().any(|s| s.contains("got int")),
            "the int narrowing must not leak through the shadow: {:?}",
            w
        );
    }

    // ---------------- Step 4: arity + unbound-symbol diagnostics ----------------

    #[test]
    fn flags_too_few_arguments() {
        // `first` expects exactly 1; 0 is wrong.
        assert!(warnings("(first)")
            .iter()
            .any(|w| w.contains("first") && w.contains("expected 1") && w.contains("got 0")));
        // `string-length` expects exactly 1.
        assert!(warnings("(string-length)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("expected 1")));
    }

    #[test]
    fn flags_too_many_arguments() {
        // `rem` is `exact(2)`; calling with 3 is wrong.
        assert!(warnings("(rem 1 2 3)")
            .iter()
            .any(|w| w.contains("rem") && w.contains("expected 2") && w.contains("got 3")));
    }

    #[test]
    fn arity_message_handles_range_and_variadic() {
        // `map-get` is `range(2, 3)` → "expected 2 to 3".
        assert!(warnings("(map-get {})")
            .iter()
            .any(|w| w.contains("map-get") && w.contains("2 to 3")));
        // `apply` is `at_least(2)` → "expected 2 or more"; 1 is too few.
        assert!(warnings("(apply f)")
            .iter()
            .any(|w| w.contains("apply") && w.contains("2 or more")));
    }

    #[test]
    fn arity_pass_is_silent_for_correct_calls() {
        assert!(warnings("(first [1 2])")
            .iter()
            .all(|w| !w.contains("number of arguments")));
        assert!(warnings("(rem 7 3)")
            .iter()
            .all(|w| !w.contains("number of arguments")));
        // Variadic: any count is fine.
        for n in 0..=5 {
            let args = (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
            let w = warnings(&format!("(+ {})", args));
            assert!(
                w.iter().all(|s| !s.contains("number of arguments")),
                "(+ {}…) should not warn arity: {:?}",
                n,
                w
            );
        }
    }

    #[test]
    fn flags_unbound_call_heads() {
        assert!(warnings("(frobnicate 1)")
            .iter()
            .any(|w| w.contains("unbound symbol: frobnicate")));
        assert!(warnings("(typo-name :hi)")
            .iter()
            .any(|w| w.contains("unbound symbol: typo-name")));
    }

    // ---- Operand / value-slot unbound symbols (whole-file mode only) --------

    #[test]
    fn flags_unbound_operand_of_a_known_call() {
        // `+` evaluates its args, so a bare unresolvable operand is unbound.
        let w = file_warnings("(defn f (x) (+ x typo))");
        assert!(
            w.iter().any(|m| m.contains("unbound symbol: typo")),
            "operand typo should be flagged: {:?}",
            w
        );
        // Through a primitive too (cons), nested under a body.
        let w = file_warnings("(defn g () (cons 1 nope))");
        assert!(
            w.iter().any(|m| m.contains("unbound symbol: nope")),
            "{:?}",
            w
        );
    }

    #[test]
    fn flags_unbound_value_in_def_let_if_slots() {
        assert!(file_warnings("(def y zilch)")
            .iter()
            .any(|m| m.contains("unbound symbol: zilch")));
        assert!(file_warnings("(defn f () (let (a absent) a))")
            .iter()
            .any(|m| m.contains("unbound symbol: absent")));
        assert!(file_warnings("(defn f () (if missing 1 2))")
            .iter()
            .any(|m| m.contains("unbound symbol: missing")));
    }

    #[test]
    fn operand_check_respects_scope_and_forward_refs() {
        // A forward reference to a later top-level def — file-global, not unbound.
        assert!(file_warnings("(defn a () (cons 1 (b)))\n(defn b () 2)")
            .iter()
            .all(|m| !m.contains("unbound")));
        // A param / let-bound name used as an operand — in scope, not unbound.
        assert!(file_warnings("(defn f (x) (+ x 1))")
            .iter()
            .all(|m| !m.contains("unbound")));
        assert!(file_warnings("(defn f () (let (y 1) (+ y 2)))")
            .iter()
            .all(|m| !m.contains("unbound")));
        // A prelude name as an operand resolves through the heap globals.
        assert!(file_warnings("(defn f () (map inc (list 1 2)))")
            .iter()
            .all(|m| !m.contains("unbound")));
    }

    #[test]
    fn operand_check_is_off_for_bare_fragments() {
        // The single-form path (REPL / `(check 'form)`) stays lenient: a free
        // operand variable is ambiguous, not provably unbound — only call *heads*
        // are flagged there. (Guards the no-false-positives rule for fragments.)
        assert!(warnings("(first xs)")
            .iter()
            .all(|m| !m.contains("unbound")));
        assert!(warnings("(+ 1 foo)").iter().all(|m| !m.contains("unbound")));
        assert!(warnings("(let (x bar) (first x))")
            .iter()
            .all(|m| !m.contains("unbound")));
    }

    #[test]
    fn flags_zero_arg_fn_passed_bare_to_an_output_sink() {
        // The `(print ansi-clear)`-for-`(print (ansi-clear))` slip: a bare
        // zero-arity global handed to print/println/str/format stringifies the
        // function (#<fn …>), never its result — silent today.
        for sink in &["print", "println", "str", "format"] {
            let w = check_with_defs(&["(defn home () \"\\e[H\")"], &format!("({} home)", sink));
            assert!(
                w.iter()
                    .any(|m| m.contains("home: function used as a value")
                        && m.contains("did you mean (home)")),
                "{} should flag a bare zero-arg fn: {:?}",
                sink,
                w
            );
        }
    }

    #[test]
    fn function_as_value_lint_is_quiet_on_the_correct_and_legitimate_shapes() {
        // Called correctly — no warning.
        assert!(
            check_with_defs(&["(defn home () \"\\e[H\")"], "(print (home))")
                .iter()
                .all(|m| !m.contains("function used as a value"))
        );
        // A fn that *takes* arguments is a plausible intentional callback value.
        assert!(check_with_defs(&["(defn f (x) x)"], "(print f)")
            .iter()
            .all(|m| !m.contains("function used as a value")));
        // A same-named *local* (not the global zero-arg fn) is left alone.
        assert!(
            check_with_defs(&["(defn home () 1)"], "(let (home 42) (print home))")
                .iter()
                .all(|m| !m.contains("function used as a value"))
        );
        // A plain value is fine.
        assert!(warnings("(print 42)")
            .iter()
            .all(|m| !m.contains("function used as a value")));
        // The lint is sink-scoped: passing a bare zero-arg fn elsewhere (a real
        // higher-order use) is not flagged.
        assert!(check_with_defs(&["(defn home () 1)"], "(map home [1 2])")
            .iter()
            .all(|m| !m.contains("function used as a value")));
    }

    #[test]
    fn unbound_is_silent_for_in_scope_names() {
        // fn/lambda params don't look unbound when used as call heads or
        // referenced in the body.
        assert!(warnings("(fn (f) (f 1 2))")
            .iter()
            .all(|w| !w.contains("unbound")));
        // let bindings: same.
        assert!(warnings("(let (g (fn (x) x)) (g 1))")
            .iter()
            .all(|w| !w.contains("unbound")));
        // Syntactic keywords aren't bound but are never "unbound".
        for src in &["(do 1 2 3)", "(when true 1)", "(cond)", "(and)", "(or)"] {
            assert!(
                warnings(src).iter().all(|w| !w.contains("unbound")),
                "syntactic keyword must not be flagged unbound: {} → {:?}",
                src,
                warnings(src)
            );
        }
    }

    #[test]
    fn unbound_is_silent_for_prelude_names() {
        // The prelude is loaded in our test heap (via Interp::new()), so
        // stdlib names resolve. `inc`, `list`, `int?`, `even?`, … are all fine.
        for src in &[
            "(inc 1)",
            "(list 1 2 3)",
            "(int? 5)",
            "(zero? 0)",
            "(map (fn (x) x) [1 2 3])",
        ] {
            assert!(
                warnings(src).iter().all(|w| !w.contains("unbound")),
                "prelude name must not be flagged unbound: {} → {:?}",
                src,
                warnings(src)
            );
        }
    }

    #[test]
    fn file_globals_make_later_forms_see_earlier_defs() {
        // `check_file` accumulates top-level def names. Without that,
        // `(my-fn 1)` in form 2 would be flagged unbound — `my-fn` isn't in
        // the heap (no eval), only in the file.
        let interp = crate::Interp::new();
        let src = "(defn my-fn (x) (+ x 1))\n(my-fn 1)";
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
        let out = check_file(&mut heap, &forms);
        let msgs: Vec<_> = out.into_iter().map(|(_, m)| m).collect();
        assert!(
            msgs.iter().all(|m| !m.contains("unbound symbol: my-fn")),
            "file-local defns must shield later calls: {:?}",
            msgs
        );
    }

    #[test]
    fn fn_params_with_rest_and_optional_dont_leak() {
        // The marker symbols `&`/`&optional` themselves are *not* binders;
        // the names that follow them are.
        assert!(warnings("(fn (x & ys) (cons x ys))")
            .iter()
            .all(|w| !w.contains("unbound")));
        assert!(warnings("(fn (x &optional (y 0)) (+ x y))")
            .iter()
            .all(|w| !w.contains("unbound")));
    }

    #[test]
    fn defn_body_sees_its_params_in_scope() {
        // A user defn whose body references its params must not flag them as
        // unbound. (The `defn` macro hasn't been expanded — the CLI checks
        // un-expanded forms — so this tests the un-expanded surface path.)
        assert!(warnings("(defn my-fn (x y) (+ x y))")
            .iter()
            .all(|w| !w.contains("unbound")));
    }

    #[test]
    fn arity_check_works_for_user_defns_in_a_real_interp() {
        // Once a defn is evaluated, its arity is derivable from its Closure.
        // `inc` (prelude) is `(defn inc (n) …)` → exact(1).
        let w = check_with_defs(&[], "(inc 1 2)");
        assert!(
            w.iter()
                .any(|s| s.contains("inc") && s.contains("expected 1")),
            "user defn arity should be enforced: {:?}",
            w
        );
    }

    // ---- Step 4 final pieces: %eq-as-guard + let-alias propagation --------
    //
    // `match` lowers `(match x (5 body) …)` to
    // `(let (m__N x) (if (%eq m__N 5) (do body) …))`. To flag a misuse on
    // `x` in `body` (where the literal pattern asserts x's type), the checker
    // needs two pieces: (1) recognise `(%eq sym lit)` as a guard asserting
    // `sym : type-of(lit)`; (2) when a `let` binds a name to another symbol,
    // propagate narrowings between the two via the alias chain.

    #[test]
    fn match_literal_pattern_narrows_the_scrutinee() {
        // `(match x (5 (first x)))` — the literal-int pattern asserts x : int;
        // `(first x)` in the body must then flag. Goes through macroexpansion
        // because `match` is a `defmacro` whose pattern compiler lowers to
        // `let`+`if`+`%eq`; the checker's narrowing rides the lowered shape.
        let w = warnings_expanded("(match x (5 (first x)) (_ nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "match int-literal pattern should narrow x: {:?}",
            w
        );
    }

    #[test]
    fn match_keyword_pattern_narrows_the_scrutinee() {
        // Mirror of the int case for a keyword literal. The scrutinee narrows to
        // the literal singleton `:foo`, so the diagnostic names that exact value.
        let w = warnings_expanded("(match x (:foo (first x)) (_ nil))");
        assert!(
            w.iter()
                .any(|s| s.contains("first") && s.contains(":foo")),
            "match keyword-literal pattern should narrow x: {:?}",
            w
        );
    }

    #[test]
    fn eq_against_a_literal_is_a_guard() {
        // The mechanism that powers match: `(%eq m 5)` in a test position
        // narrows `m` to `:int` in the then-branch. (Symmetric — both
        // `(%eq m 5)` and `(%eq 5 m)` should narrow.)
        let w = warnings("(if (%eq m 5) (first m) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "%eq with sym + literal should narrow: {:?}",
            w
        );
        let w = warnings("(if (%eq 5 m) (first m) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "%eq with literal + sym (reversed) should narrow: {:?}",
            w
        );
    }

    #[test]
    fn eq_between_two_variables_is_not_a_guard() {
        // Equality between two unknowns asserts nothing about either's type.
        // No false positive must fire on the body.
        let w = warnings("(if (%eq a b) (first a) nil)");
        assert!(
            w.iter().all(|s| !s.contains("first")),
            "%eq between two vars should not narrow: {:?}",
            w
        );
    }

    #[test]
    fn eq_guard_does_not_narrow_the_else_branch() {
        // `(= m "x")` being *false* does NOT prove `m` isn't a string — it could
        // be another string. So the else-branch must not narrow `m` to `¬string`
        // and flag a valid `(string-length m)`. (Same then-only soundness as the
        // `and` guard.)
        let w = warnings(r#"(if (%eq m "x") :yes (string-length m))"#);
        assert!(
            w.iter().all(|s| !s.contains("string-length")),
            "the else-branch of an `=`/`%eq` guard must not be narrowed: {w:?}"
        );
        // The then-branch must still narrow (sanity): `(= m 5)` true ⇒ m : int.
        let w = warnings("(if (%eq m 5) (first m) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "the then-branch must still narrow m to int: {w:?}"
        );
    }

    #[test]
    fn let_alias_propagates_narrowing_in_both_directions() {
        // The match pattern compiler's exact shape: alias `m` to `x`, then
        // narrow `m` via a guard. The narrowing must flow back onto `x` so a
        // body that uses `x` (not `m`) still sees the asserted type.
        let w = warnings("(let (m x) (if (int? m) (first x) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "let-alias should propagate narrowing from m to x: {:?}",
            w
        );
        // And the symmetric direction: narrow x, alias-narrows m.
        let w = warnings("(let (m x) (if (int? x) (first m) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "let-alias should propagate narrowing from x to m: {:?}",
            w
        );
    }

    #[test]
    fn shadowing_clears_an_alias() {
        // An inner let that rebinds an aliased name to something else breaks
        // the chain — the new binding is the new name's type, no alias.
        // `(let (m x) (let (m 5) (first m)))` flags the inner `(first m)`
        // because `m` is now int, but that's via the literal-type binding,
        // not the broken alias.
        let w = warnings("(let (m x) (let (m 5) (first m)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "shadowed let should still warn on the inner int: {:?}",
            w
        );
        // The outer `x` must not be narrowed by the inner shadowing.
        let w = warnings("(let (m x) (let (m 5) (println x)))");
        assert!(
            w.iter().all(|s| !s.contains("first")),
            "shadowing must not leak narrowing back to the original: {:?}",
            w
        );
    }

    // ---- callback-arity check over higher-order combinators (ADR-078) ----

    #[test]
    fn flags_a_named_callback_of_the_wrong_arity() {
        // `cons` is arity 2; `map` calls its callback with 1 arg → real bug.
        let w = warnings("(map cons nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("map") && s.contains("callback") && s.contains("cons")),
            "map should flag a 2-arg callback called with 1: {w:?}"
        );
    }

    #[test]
    fn accepts_a_named_callback_of_the_right_arity() {
        // `inc` is arity 1 — exactly what `map` supplies. No warning.
        let w = warnings("(map inc nil)");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "a correct-arity callback must not warn: {w:?}"
        );
        // A variadic callback (`+` accepts 1) is fine too.
        let w = warnings("(map + nil)");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "a variadic callback must not warn: {w:?}"
        );
    }

    #[test]
    fn flags_a_lambda_callback_of_the_wrong_arity() {
        // A 2-param lambda passed where `map` calls it with 1 arg.
        let w = warnings("(map (fn (a b) a) nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("map") && s.contains("callback") && s.contains("the lambda")),
            "map should flag a 2-arg lambda: {w:?}"
        );
        // Correct arity — no warning.
        let w = warnings("(map (fn (a) a) nil)");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "a 1-arg lambda must not warn under map: {w:?}"
        );
    }

    #[test]
    fn lambda_head_behaves_like_fn() {
        // `lambda` is a synonym for `fn` (and survives macro expansion as itself),
        // so the callback-arity check must see through it exactly like `fn`.
        let w = warnings("(map (lambda (a b) a) nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("map") && s.contains("callback") && s.contains("the lambda")),
            "map should flag a 2-arg `lambda` callback: {w:?}"
        );
        let w = warnings("(map (lambda (a) a) nil)");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "a 1-arg `lambda` must not warn under map: {w:?}"
        );
    }

    #[test]
    fn lambda_form_is_not_unbound() {
        // Regression: `lambda` was missing from SPECIAL_HEAD / is_syntactic_keyword,
        // so whole-file mode flagged the head AND its params as unbound symbols — a
        // false positive on perfectly valid code.
        let w = file_warnings("(def f (map (lambda (x) (+ x 1)) (list 1 2 3)))");
        assert!(
            w.iter().all(|m| !m.contains("unbound symbol")),
            "a `lambda` literal must not draw unbound-symbol warnings: {w:?}"
        );
    }

    // ---- gradual-assignment check: `(def x …)` vs a non-arrow `(sig x T)` ----
    // (GradualTy's first consumer — ADR-024.)

    #[test]
    fn def_against_value_sig_flags_a_literal_mismatch() {
        // `(sig n int)` then `(def n "hello")` — a precise literal disjoint from
        // the declared type. stat(string) ⊄ int → flagged.
        let w = file_warnings(r#"(sig n int) (def n "hello")"#);
        assert!(
            w.iter().any(|m| m.contains("n: value of type string")
                && m.contains("not assignable")
                && m.contains("int")),
            "a string literal assigned to an int-declared name must warn: {w:?}"
        );
    }

    #[test]
    fn def_against_value_sig_catches_a_bounded_dynamic_global() {
        // The genuine GradualTy value-add: `label` is a redefinable global with a
        // declared type, so it's dynamic_within(string) — a bounded dynamic that
        // Option<Ty> can't represent. Assigning it to an int-declared name is
        // disjoint (string ∩ int = ⊥) → flagged.
        let w = file_warnings(
            r#"(sig count int) (sig label string) (def label "x") (def count label)"#,
        );
        assert!(
            w.iter()
                .any(|m| m.contains("count: value of type string") && m.contains("int")),
            "a string-typed global assigned to an int-declared name must warn: {w:?}"
        );
    }

    #[test]
    fn def_against_value_sig_defers_when_consistent_or_unknown() {
        // Every one of these is consistent (or dynamic) → no assignment warning.
        for src in [
            "(sig n int) (def n 5)",                          // exact
            "(sig m number) (def m 5)",                       // int <: number
            "(sig n int) (def n (+ 1 2))",                    // call result widened → defer
            "(sig n int) (def n some-unknown-global)",        // unknown → pure dynamic
            "(sig a int) (sig b number) (def b 5) (def a b)", // int <- number: ∩≠⊥ → defer
        ] {
            let w = file_warnings(src);
            assert!(
                w.iter().all(|m| !m.contains("not assignable")),
                "a consistent/dynamic assignment must not warn ({src}): {w:?}"
            );
        }
    }

    #[test]
    fn declared_return_type_mismatch_is_flagged() {
        // Body yields a number, declared return is string → disjoint → flagged.
        let w = file_warnings("(sig f (int -> string)) (defn f (x) (+ x 1))");
        assert!(
            w.iter()
                .any(|m| m.contains("f: declared return type string")
                    && m.contains("yields number")),
            "a number body vs a string return must warn: {w:?}"
        );
        // A literal body mismatch too.
        let w = file_warnings(r#"(sig g (int -> int)) (defn g (x) "hello")"#);
        assert!(
            w.iter().any(|m| m.contains("g: declared return type int") && m.contains("string")),
            "a string-literal body vs an int return must warn: {w:?}"
        );
    }

    #[test]
    fn wider_sig_param_returned_as_narrower_is_flagged() {
        // A sig-typed param carries its exact contract type, so returning a
        // `number` param where the declared return is `int` is caught via the
        // precise `⊆` path — the first non-disjoint ("merely wider") mismatch the
        // disjointness checker structurally can't produce.
        let w = file_warnings("(sig f (number -> int)) (defn f (x) x)");
        assert!(
            w.iter()
                .any(|m| m.contains("f: declared return type int") && m.contains("number")),
            "a number param returned as int must warn: {w:?}"
        );
        // Same or narrower param, and a param narrowed by a guard, must not warn.
        for src in [
            "(sig g (int -> int)) (defn g (x) x)",
            "(sig h (int -> number)) (defn h (x) x)",
            "(sig k (number -> int)) (defn k (x) (if (int? x) x 0))",
        ] {
            let w = file_warnings(src);
            assert!(
                w.iter().all(|m| !m.contains("return type")),
                "a consistent/narrowed param return must not warn ({src}): {w:?}"
            );
        }
    }

    #[test]
    fn declared_return_type_defers_when_consistent() {
        // (+ x 1) : number — int <: number and number ∩ int ≠ ⊥, so neither of
        // these declared returns warns (a widened body never over-warns).
        for src in [
            "(sig inc (int -> int)) (defn inc (x) (+ x 1))",
            "(sig h (int -> number)) (defn h (x) (+ x 1))",
            "(sig id (int -> int)) (defn id (x) x)",
        ] {
            let w = file_warnings(src);
            assert!(
                w.iter().all(|m| !m.contains("return type")),
                "a consistent return must not warn ({src}): {w:?}"
            );
        }
    }

    #[test]
    fn declared_global_type_flows_into_value_position() {
        // `(sig g int)` makes `g`'s declared type visible where it's used, so a
        // disjoint use is caught — even though `g` is a redefinable global.
        let w = file_warnings("(sig g int) (def g 5) (def r (string-length g))");
        assert!(
            w.iter()
                .any(|m| m.contains("string-length") && m.contains("int")),
            "a declared int global used where a string is wanted must warn: {w:?}"
        );
        // A compatible use defers (int ⊆ number).
        let w = file_warnings("(sig g int) (def g 5) (def r (+ 1 g))");
        assert!(
            w.iter().all(|m| !m.contains("expects number")),
            "a declared int global is fine for +: {w:?}"
        );
    }

    #[test]
    fn unknown_module_qualified_name_is_not_unbound() {
        // A qualified reference whose module isn't loaded — defined dynamically
        // (`%load-string`, a required temp module) or in a file a single-file check
        // didn't load — can't be proven unbound, so it's left alone.
        for src in [
            "(some-unloaded-mod/thing 1)",
            "(a/b/c/deep-thing 1)",
            "(+ 1 other-mod/value)",
        ] {
            let w = file_warnings(src);
            assert!(
                w.iter().all(|m| !m.contains("unbound symbol")),
                "an unknown-module qualified name must not be flagged ({src}): {w:?}"
            );
        }
        // But a typo in a *known* module (some `mod/*` is loaded) is still flagged:
        // requiring `test` makes `test/` a known prefix.
        let w = file_warnings("(require 'test) (test/no-such-fn 1)");
        assert!(
            w.iter().any(|m| m.contains("unbound symbol: test/no-such-fn")),
            "a typo in a known module must still be flagged: {w:?}"
        );
    }

    #[test]
    fn unexpandable_macro_calls_dont_false_flag() {
        // A file-local macro the checker can't expand: its arguments are opaque
        // syntax. (a) A macro that `def`s its symbol arg — the name must not look
        // unbound later. (b) A macro that splices an arg into a binder — the
        // spliced names must not look unbound.
        let a = file_warnings(
            "(defmacro mk (n) `(def ~n (fn (x) x))) (mk qf) (qf 5)",
        );
        assert!(
            a.iter().all(|m| !m.contains("unbound symbol")),
            "a macro-defined name must not look unbound: {a:?}"
        );
        let b = file_warnings(
            "(defmacro wp (v & body) `(let ((a b) ~v) ~@body)) (wp [1 2] (+ a b))",
        );
        assert!(
            b.iter().all(|m| !m.contains("unbound symbol")),
            "names a macro splices into a binder must not look unbound: {b:?}"
        );
        // A genuine typo under a *known* (arg-evaluating) callee is still flagged.
        let c = file_warnings("(println (genuine-typo 5))");
        assert!(
            c.iter().any(|m| m.contains("unbound symbol: genuine-typo")),
            "a real unbound call head must still be flagged: {c:?}"
        );
    }

    #[test]
    fn transient_is_a_valid_count_and_contains_arg() {
        // count/length/contains? dispatch to transient-* kernel hooks at runtime, so
        // a live transient is a valid argument — the sigs must admit Tag::Transient.
        for src in [
            "(count (transient {}))",
            "(length (transient {}))",
            "(contains? (transient {}) :k)",
        ] {
            let w = warnings(src);
            assert!(
                w.iter().all(|m| !m.contains("expects")),
                "transient must be accepted by {src}: {w:?}"
            );
        }
        // A genuinely wrong arg (a number) is still flagged — the domain stays tight.
        assert!(warnings("(count 5)").iter().any(|m| m.contains("count")));
    }

    #[test]
    fn multi_arity_fn_clause_params_are_bound() {
        // Regression: `check_fn` read a multi-arity fn's first clause as a param
        // list, so a param used only in a *later* clause looked unbound — a false
        // positive (it fired identically for `fn` and `lambda`).
        let w = file_warnings("(def g (fn ((a) (* a 2)) ((a b) (+ a b))))");
        assert!(
            w.iter().all(|m| !m.contains("unbound symbol")),
            "multi-arity fn clause params must not look unbound: {w:?}"
        );
        // `defn` (which expands to `(def name (fn …))`) and `lambda` too.
        let w = file_warnings("(defn h ((a) a) ((a b) (+ a b)))");
        assert!(w.iter().all(|m| !m.contains("unbound symbol")), "defn: {w:?}");
        let w = file_warnings("(def k (lambda ((a) a) ((a b) (+ a b))))");
        assert!(w.iter().all(|m| !m.contains("unbound symbol")), "lambda: {w:?}");
    }

    #[test]
    fn self_recursive_let_bound_closure_is_bound() {
        // Regression: a `let`-bound `fn`/`lambda` that calls its own binding name
        // resolves at runtime (the closure captures the frame, late-binds on call),
        // but the checker flagged the self-reference unbound. Pre-binding fn-valued
        // let names fixes it — for `let` and `let*`, `fn` and `lambda`.
        let w = file_warnings("(defn t () (let (fac (fn (n) (if (= n 0) 1 (fac n)))) (fac 5)))");
        assert!(
            w.iter().all(|m| !m.contains("unbound symbol: fac")),
            "self-recursive let closure must not look unbound: {w:?}"
        );
        let w =
            file_warnings("(defn t () (let* (fac (lambda (n) (if (= n 0) 1 (fac n)))) (fac 5)))");
        assert!(
            w.iter().all(|m| !m.contains("unbound symbol: fac")),
            "self-recursive let* lambda must not look unbound: {w:?}"
        );
        // But an *eager* forward reference in a non-closure RHS still surfaces.
        let w = file_warnings("(defn t () (let (a undefined-thing b 1) a))");
        assert!(
            w.iter().any(|m| m.contains("unbound symbol: undefined-thing")),
            "an eager forward/undefined reference must still be flagged: {w:?}"
        );
    }

    #[test]
    fn reduce_and_fold_expect_a_two_arg_callback() {
        // reduce/fold call `(f acc x)` — 2 args. A 1-arg callback is wrong.
        let w = warnings("(reduce (fn (a) a) 0 nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("reduce") && s.contains("callback")),
            "reduce should flag a 1-arg callback: {w:?}"
        );
        let w = warnings("(fold inc 0 nil)");
        assert!(
            w.iter()
                .any(|s| s.contains("fold") && s.contains("callback")),
            "fold should flag a 1-arg callback (inc): {w:?}"
        );
        // A correct 2-arg callback is silent.
        let w = warnings("(reduce (fn (a b) a) 0 nil)");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "a 2-arg callback must not warn under reduce: {w:?}"
        );
    }

    #[test]
    fn callback_arity_is_skipped_when_unknown() {
        // A multi-arity lambda accepts 1 *and* 2 — must not warn (we bail rather
        // than risk a false positive).
        let w = warnings("(map (fn ((a) a) ((a b) a)) nil)");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "multi-arity lambda must be skipped: {w:?}"
        );
        // A locally-bound callback has unknown arity here — skip.
        let w = warnings("(fn (f) (map f nil))");
        assert!(
            w.iter().all(|s| !s.contains("callback")),
            "a local callback must be skipped: {w:?}"
        );
    }

    // ---- element types flow through first/last/nth (ADR-078 slice 2) ----

    #[test]
    fn first_of_a_string_vector_is_not_a_number() {
        // `(first ["a" "b"])` : string | nil — disjoint from number → flagged.
        let w = warnings(r#"(+ 1 (first ["a" "b"]))"#);
        assert!(
            w.iter().any(|s| s.contains("+") && s.contains("string")),
            "expected a number/string mismatch from the element type: {w:?}"
        );
    }

    #[test]
    fn first_of_an_int_vector_is_a_number() {
        // `(first [10 20])` : int | nil — overlaps number → no warning.
        let w = warnings("(+ 1 (first [10 20]))");
        assert!(
            w.iter().all(|s| !s.contains("expects number")),
            "an int element must not warn against +: {w:?}"
        );
    }

    #[test]
    fn list_constructor_carries_its_element_type() {
        // `(list "a" "b")` : list<string>, so `(first …)` is string|nil.
        let w = warnings(r#"(+ 1 (first (list "a" "b")))"#);
        assert!(
            w.iter().any(|s| s.contains("+") && s.contains("string")),
            "(list …) element type should flow to first: {w:?}"
        );
    }

    #[test]
    fn heterogeneous_or_unknown_elements_do_not_warn() {
        // Mixed elements → int|string element; first → int|string|nil, which
        // overlaps number → no false positive.
        let w = warnings(r#"(+ 1 (first [1 "a"]))"#);
        assert!(
            w.iter().all(|s| !s.contains("expects number")),
            "a heterogeneous element type must not warn: {w:?}"
        );
        // first of an unknown (variable) sequence → unknown → no warning.
        let w = warnings("(fn (xs) (+ 1 (first xs)))");
        assert!(
            w.iter().all(|s| !s.contains("expects number")),
            "an unknown sequence must not warn: {w:?}"
        );
    }

    // ---- `and`-guard narrowing in an `if` test (the match-lowering fix) ----

    #[test]
    fn and_guard_narrows_in_the_then_branch() {
        // `(and (int? x) …)` as an `if` test must narrow `x` to int in the then
        // branch — so a use that would mismatch the *original* type is suppressed
        // (here `x` is a string, narrowed to never → the `+` use is unreachable).
        let w = warnings_expanded(r#"(let (x "s") (if (and (int? x) true) (+ x 1) 0))"#);
        assert!(
            w.iter().all(|s| !s.contains("expects number")),
            "an `and` guard should narrow x in the then branch: {w:?}"
        );
    }

    #[test]
    fn matching_a_list_against_a_vector_pattern_is_not_flagged() {
        // The match compiler lowers a vector pattern to
        // `(if (and (vector? m) (= (vector-length m) 2)) (… (vector-ref m i) …) …)`.
        // With `(list 1 2)` now typed `list<int>`, the guarded `vector-ref` must
        // not be flagged — the `and` guard narrows `m` to a vector (→ never here).
        let w = warnings_expanded("(match (list 1 2) ([a b] :vec) (_ :not-vec))");
        assert!(
            w.iter()
                .all(|s| !s.contains("vector-ref") && !s.contains("vector-length")),
            "a list matched against a vector pattern must not warn: {w:?}"
        );
    }

    #[test]
    fn and_guard_does_not_narrow_the_else_branch() {
        // A falsy `(and (vector? m) …)` does NOT imply `m` isn't a vector — a
        // *later* conjunct may have failed. So the else-branch must keep `m`'s
        // full type; flagging a vector op there would be a false positive.
        let w = warnings_expanded(
            "(fn (m) (if (and (vector? m) (%eq (vector-length m) 2)) \
                         (vector-ref m 0) (vector-ref m 0)))",
        );
        assert!(
            w.iter().all(|s| !s.contains("vector-ref")),
            "the else-branch of an `and` guard must not be narrowed: {w:?}"
        );
        // The then-branch still narrows (sanity: the guard didn't go silent).
        let w = warnings_expanded(r#"(fn (m) (if (and (int? m) true) (string-length m) 0))"#);
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "the then-branch should still narrow m to int: {w:?}"
        );
    }

    #[test]
    fn or_guard_does_not_falsely_narrow() {
        // `or` must NOT narrow from its first operand (a truthy `or` implies
        // nothing about it). `(or (int? x) true)` is always true, so the then
        // branch keeps `x`'s full (string) type — and a genuine misuse there is
        // still seen. (Guards against the `and`-fix over-reaching into `or`.)
        let w = warnings_expanded(r#"(let (x "s") (if (or (int? x) true) (string-length x) 0))"#);
        assert!(
            w.iter().all(|s| !s.contains("expects")),
            "a correct use under an `or` guard must not warn: {w:?}"
        );
    }

    // ---- parametric HOF result types — map / filter (ADR-078, Option B) ----

    #[test]
    fn map_result_flows_the_callback_return() {
        // `(map inc (list 1 2 3))` : list<number>, so `(first …)` is number|nil —
        // disjoint from string → string-length flags it.
        let w = warnings("(string-length (first (map inc (list 1 2 3))))");
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "map's element type (number) should flow to first: {w:?}"
        );
        // ...and a numeric sink is fine (number overlaps).
        let w = warnings("(+ 1 (first (map inc (list 1 2 3))))");
        assert!(
            w.iter().all(|s| !s.contains("expects")),
            "a number element must not warn against +: {w:?}"
        );
    }

    #[test]
    fn filter_preserves_the_element_type() {
        // `(filter even? (list 1 2 3))` : list<int> — element type unchanged.
        let w = warnings("(string-length (first (filter even? (list 1 2 3))))");
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "filter should preserve the int element type: {w:?}"
        );
    }

    #[test]
    fn element_type_flows_through_more_combinators() {
        // Structured-types extension: second/third/rest/but-last/distinct/dedupe/
        // take-last/drop-last/remove/keep/interpose/range all flow the element type,
        // so a downstream string-vs-number mismatch is caught. Each must warn here.
        for src in [
            r#"(+ 1 (second ["a" "b"]))"#,
            r#"(+ 1 (first (rest ["a" "b"])))"#,
            r#"(+ 1 (first (but-last ["a" "b"])))"#,
            r#"(+ 1 (first (distinct ["a" "b"])))"#,
            r#"(+ 1 (first (dedupe ["a" "b"])))"#,
            r#"(+ 1 (first (remove (fn (x) false) ["a" "b"])))"#,
            r#"(+ 1 (first (take-last 1 ["a" "b"])))"#,
            r#"(+ 1 (first (keep (fn (x) x) ["a" "b"])))"#,
            "(string-length (first (range 5)))",
        ] {
            let w = warnings(src);
            assert!(
                w.iter().any(|s| s.contains("number") || s.contains("string")),
                "expected an element-type mismatch for {src}: {w:?}"
            );
        }
        // Negative controls — a valid element type must NOT warn.
        for src in [
            "(+ 1 (second [10 20]))",
            "(+ 1 (first (rest [10 20])))",
            // interpose unions the separator: int|string includes int → valid for +.
            r#"(+ 1 (first (interpose "z" [1 2])))"#,
        ] {
            let w = warnings(src);
            assert!(
                w.iter().all(|s| !s.contains("expects number")),
                "a valid element type must not warn for {src}: {w:?}"
            );
        }
    }

    #[test]
    fn identity_lambda_preserves_element_type() {
        // `(map (fn (x) x) (list 1 2 3))` : list<int> — the lambda returns its
        // argument, so B = the element type A.
        let w = warnings("(string-length (first (map (fn (x) x) (list 1 2 3))))");
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "an identity callback should preserve the element type: {w:?}"
        );
    }

    #[test]
    fn map_filter_do_not_refine_when_uncertain() {
        // Unknown callback (a local) → no refinement → no warning.
        let w = warnings("(fn (g) (string-length (first (map g (list 1 2 3)))))");
        assert!(
            w.iter().all(|s| !s.contains("string-length")),
            "an unknown callback must not refine the result: {w:?}"
        );
        // Identity callback + unknown collection → B depends on the (unknown)
        // element type → no refinement.
        let w = warnings("(fn (xs) (string-length (first (map (fn (x) x) xs))))");
        assert!(
            w.iter().all(|s| !s.contains("string-length")),
            "an identity callback over an unknown collection must not refine: {w:?}"
        );
        // Branchy lambda body → can't type it → bail to flat (no false positive).
        let w = warnings(r#"(string-length (first (map (fn (x) (if x 1 "a")) (list 1 2 3))))"#);
        assert!(
            w.iter().all(|s| !s.contains("string-length")),
            "a branchy lambda body must bail to a flat result: {w:?}"
        );
    }

    // ---- reduce / fold result types (slice 2) ----

    #[test]
    fn reduce_result_is_the_accumulator_type() {
        // `(reduce + 0 (list 1 2 3))` : number (init int ∪ +'s number return) —
        // disjoint from string → flagged.
        let w = warnings("(string-length (reduce + 0 (list 1 2 3)))");
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "reduce's accumulator type should flow out: {w:?}"
        );
        // ...and a numeric sink is fine.
        let w = warnings("(+ 1 (reduce + 0 (list 1 2 3)))");
        assert!(
            w.iter().all(|s| !s.contains("expects")),
            "a numeric reduce result must not warn against +: {w:?}"
        );
    }

    #[test]
    fn fold_with_a_lambda_callback_types_the_result() {
        // `(fold (fn (acc x) (+ acc x)) 0 …)` : number — the 2-arg callback's
        // return (number) joined with the init (int).
        let w = warnings("(string-length (fold (fn (acc x) (+ acc x)) 0 (list 1 2 3)))");
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "fold should type the accumulator from a lambda callback: {w:?}"
        );
    }

    #[test]
    fn reduce_fold_bail_when_init_or_callback_unknown() {
        // Unknown callback (local) → flat, no warning.
        let w = warnings("(fn (g) (string-length (reduce g 0 (list 1 2 3))))");
        assert!(
            w.iter().all(|s| !s.contains("string-length")),
            "an unknown reduce callback must not refine: {w:?}"
        );
        // Unknown init type (a fn param) → flat, no warning.
        let w = warnings("(fn (init) (string-length (reduce + init (list 1 2 3))))");
        assert!(
            w.iter().all(|s| !s.contains("string-length")),
            "an unknown init must not refine the reduce result: {w:?}"
        );
    }
}

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
mod soundness_oracle {
    use super::ctx::Ctx;
    use super::guards::expr_ty;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};
    use crate::types::Ty;

    /// Does runtime value `v` belong to the set denoted by `ty`? Tag membership,
    /// plus a recursive element check when `ty` pins a sequence element type. (An
    /// arrow refinement isn't structurally checkable on a live closure here, so
    /// for functions the oracle asserts only tag membership — `fn` / `native`.)
    fn value_member_of(heap: &Heap, v: Value, ty: &Ty) -> bool {
        if !ty.contains_tag(value::tag(v)) {
            return false;
        }
        if let Some(elem) = ty.elem_ty() {
            match v {
                Value::Vector(id) => {
                    for it in heap.vector(id).to_vec() {
                        if !value_member_of(heap, it, elem) {
                            return false;
                        }
                    }
                }
                Value::Pair(_) => {
                    let mut cur = v;
                    while let Value::Pair(p) = cur {
                        let (h, t) = heap.pair(p);
                        if !value_member_of(heap, h, elem) {
                            return false;
                        }
                        cur = t;
                    }
                }
                _ => {}
            }
        }
        true
    }

    #[test]
    fn expr_ty_is_a_sound_overapproximation_of_runtime_values() {
        // Each entry is a closed expression whose static `expr_ty` is `Some`.
        // Concentrated on the refinement-producing rules (literals, constructors,
        // extractors, higher-order results) — where an under-approximation hides.
        let cases = [
            // literals
            "5",
            "3.0",
            "\"hi\"",
            ":k",
            "true",
            "false",
            "nil",
            // vector literals (element union)
            "[1 2 3]",
            "[1 \"a\" :k]",
            "[]",
            // quote
            "(quote sym)",
            "(quote (1 2 3))",
            // primitive results
            "(string-length \"hi\")",
            "(+ 1 2)",
            "(- 10 3 2)",
            "(* 2 3)",
            "(< 1 2)",
            "(<= 1 1)",
            "(string->number \"5\")",
            // sequence constructors / extractors
            "(list 1 2 3)",
            "(vector 1 2 3)",
            "(first [1 2 3])",
            "(last [1 2 3])",
            "(nth [10 20 30] 1)",
            "(first [])",
            // higher-order results (parametric — ADR-078)
            "(map inc [1 2 3])",
            "(filter even? [1 2 3 4])",
            "(reduce + 0 [1 2 3])",
            "(fold (fn (a x) (+ a x)) 0 [1 2 3])",
            "(map (fn (x) (+ x 1)) [1 2 3])",
            // empty / all-filtered results evaluate to `nil` — these exercise the
            // `… | nil` widening in `list_result`; drop it and the oracle bites.
            "(map inc [])",
            "(filter (fn (x) false) [1 2 3])",
            // nested
            "(first (map inc [1 2 3]))",
            "(reduce + 0 (map inc [1 2 3]))",
        ];
        for src in cases {
            let mut interp = crate::Interp::new();
            // Static type of the form (read in this heap, typed in the empty ctx).
            let form = crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse");
            let Some(t) = expr_ty(&interp.heap, form, &Ctx::default()) else {
                continue; // checker makes no claim → nothing to verify
            };
            // Runtime value of the same source (fresh parse + eval).
            let v = interp.eval_str(src).expect("eval");
            assert!(
                value_member_of(&interp.heap, v, &t),
                "UNSOUND: {src} : static `{t}`, but the runtime value {} (tag {}) \
                 is not a member of it",
                crate::syntax::printer::print(&interp.heap, v),
                value::tag(v).name(),
            );
        }
    }

    #[test]
    fn correct_programs_draw_no_type_disjointness_warning() {
        // Facet (II): every program here EVALUATES cleanly (no runtime type
        // error), so any `expects … got` / callback-arity warning the checker
        // emits on it would be a false positive. Concentrated on the guard /
        // narrowing shapes (`if`, `match`, the `and`-short-circuit vector pattern)
        // — the path B1's `negate` over-approximation protects.
        let cases = [
            "(+ 1 (first [1 2 3]))",
            "(string-length (str 1 2 3))",
            "(if (int? 5) (+ 5 1) :no)",
            "(if (number? 5) (* 5 5) :no)",
            "(let (x [1 2 3]) (if (vector? x) (first x) :no))",
            "(let (x 5) (if (int? x) (+ x 1) x))",
            "(map inc [1 2 3])",
            "(map (fn (n) (+ n 1)) [1 2 3])",
            "(reduce + 0 [1 2 3])",
            "(filter even? [1 2 3 4])",
            "(first (map inc [1 2 3]))",
            "(match 5 (5 (+ 5 1)) (_ 0))",
            "(match [1 2] ([a b] (+ a b)) (_ 0))",
        ];
        for src in cases {
            let mut interp = crate::Interp::new();
            // It must actually run cleanly — that's what makes a warning a false positive.
            interp
                .eval_str(src)
                .unwrap_or_else(|e| panic!("`{src}` should evaluate cleanly: {e:?}"));
            // Then check the macro-expanded form, like the real pre-flight does.
            let form = crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse");
            let form =
                crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
            let bad: Vec<String> = super::check_form(&interp.heap, form)
                .into_iter()
                .filter(|w| w.contains("expects") || w.contains("callback called with"))
                .collect();
            assert!(bad.is_empty(), "FALSE POSITIVE on correct `{src}`: {bad:?}");
        }
    }
}
