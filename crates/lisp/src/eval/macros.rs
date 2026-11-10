//! Macro support: quasiquote expansion and `macroexpand`. Heap-threaded.
//!
//! Syntax (Clojure-style): `` `tmpl `` quotes, `~x` splices a value, `~@xs`
//! splices the elements of a sequence. Nested quasiquote is not level-tracked
//! (v0.1) — unquotes resolve at the first enclosing quasiquote.
//!
//! Quasiquote is a **compile-time / eval-time code transform**, not a runtime
//! walker: [`expand_quasiquote`] rewrites a template into *builder code*
//! (`` `(a ~b ~@c) `` → `(append (list 'a) (list b) c)`), and the normal
//! evaluator runs that. The transform never re-enters `eval`, so — unlike the
//! old walker, which evaluated unquotes inline while accumulating LOCAL
//! transients in Rust (the GC-rooting hazard, ADR-084) — it hits no safepoint
//! and needs no operand-stack rooting; the unquoted sub-forms are rooted by the
//! evaluator as ordinary `list`/`append` operands.
//!
//! The compile pass's other jobs live in children: [`hygiene`] (template binders),
//! [`resolve`] (namespace resolution and `compile`), [`template_escape`] (a macro
//! template's call heads pinned to root, ADR-423), [`escape_capture`] (a local never
//! captures a `/name` escape), [`pipeline`] and [`linmap`] (the
//! compile-time loop rewrites) and [`patterns`] (pattern-binder lowering).

mod escape_capture;
mod hygiene;
mod linmap;
mod patterns;
mod pipeline;
mod resolve;
mod template_escape;
mod typed;

use escape_capture::*;
use hygiene::*;
use linmap::*;
pub(crate) use patterns::*;
use pipeline::*;
pub use resolve::*;
use template_escape::*;
use typed::*;
pub(crate) use typed::{defn_param_types, typed_param_sig};

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, ClosureId, EnvId, Symbol, Value, ValueRef};
use crate::error::{LispError, LispResult};
use std::collections::{HashMap, HashSet};

/// Bound on recursion depth for the quasiquote walker and the compile pass.
/// Past this, return `LispError::runtime` rather than overflowing the native
/// Rust stack — a deeply nested template, vector, or map (from a user file or
/// a misbehaving macro) should produce a clean error, not abort the process.
// A `case`/`cond`/`match`/`receive` lowers to one nested level PER ARM, so the cap must
// admit a flat several-hundred-arm dispatch. The walkers that recurse per level grow their
// stack (`stacker`) rather than trust the cap; the cap is a bound on runaway expansion.
const MAX_DEPTH: u32 = 1024;

/// Bound on `macroexpand`'s head-fixpoint *rounds* — a different quantity from
/// the nesting depth above (it counts successive whole-form rewrites, not
/// recursion), kept as its own constant so tuning one never silently retunes
/// the other. The prelude's Brood-level `macroexpand` mirrors this value
/// (`macroexpand--max-rounds` in `std/prelude.blsp`).
pub(crate) const MAX_EXPAND_ROUNDS: u32 = 1024;

/// Per-expansion auto-gensym table (Clojure-style `x#`). Maps a literal template
/// symbol whose name ends in `#` to a single fresh gensym, so every occurrence of
/// that name *within one backtick expansion* refers to the same fresh symbol — and
/// two expansions (or two macro uses) get distinct ones. Holds only interned
/// `Value::Sym`s and `Symbol` (`u32`) keys, both GC-immune (symbols never move and
/// ship by name), so it needs no operand-stack rooting even though quasiquote runs
/// with collection enabled (ADR-061). See `maybe_autogensym`.
type AutoGen = HashMap<Symbol, Value>;

/// Expand a quasiquote template into **builder code** — a pure structural
/// transform that never re-enters `eval`. `` `(a ~b ~@c) `` becomes
/// `(append (list 'a) (list b) c)`; evaluating that builder code reconstructs
/// the template with `~unquote` values inlined and `~@unquote-splicing`
/// sequences spliced. Replaces the old runtime walker (`quasiquote_depth`),
/// which evaluated unquotes inline while holding LOCAL transients in Rust — the
/// GC-rooting hazard. Here the unquoted forms become operands of `list`/`append`
/// that the normal evaluator roots, and this transform itself touches no
/// safepoint (it calls no `eval`), so its own transients are stable without
/// rooting.
///
/// Auto-gensym (`x#`) resolves to a fresh symbol here, once per template symbol
/// per expansion. The enclosing macro body is re-evaluated on every application,
/// so each expansion gets distinct gensyms — Clojure-style binding hygiene.
pub fn expand_quasiquote(heap: &mut Heap, template: Value) -> LispResult {
    // Automatic binding hygiene (ADR-066 amendment): alpha-rename the template's own
    // `let`/`fn` binders to fresh gensyms so they can't capture spliced caller code.
    // A no-op for a binder-free template. Runs before `qq_elem` builds the template.
    let template = hygiene_rename(heap, template);
    let mut autogen = AutoGen::new();
    qq_elem(heap, template, 0, 1, &mut autogen)
}

/// Clojure-style auto-gensym: a literal template symbol whose name ends in `#`
/// (e.g. `tmp#`) becomes a fresh gensym, consistently for every occurrence within
/// one backtick expansion (tracked in `autogen`). This is opt-in binding hygiene —
/// a macro-introduced binding named `tmp#` can neither capture nor be captured by
/// the caller's `tmp`. Only *literal* template symbols reach here; symbols inside
/// `~unquote` go through `eval` instead, so a user's `x#` in unquoted code is left
/// alone. A bare `#` (no prefix) is not rewritten.
fn maybe_autogensym(v: Value, autogen: &mut AutoGen) -> Value {
    if let ValueRef::Sym(s) = v.unpack() {
        let name = value::symbol_name_ref(s);
        if name.len() > 1 && name.ends_with('#') {
            return *autogen
                .entry(s)
                .or_insert_with(|| value::gensym(&name[..name.len() - 1]));
        }
    }
    v
}

/// A symbol Value for a builder-code head the transform emits (`list`,
/// `append`, `vector`, `hash-map`, `apply`). Interning dedups, so this is cheap.
fn sym(name: &str) -> Value {
    Value::symbol(value::intern(name))
}

/// `(quote v)` — the builder form that reproduces a literal symbol/atom datum.
fn quote_form(heap: &mut Heap, v: Value) -> Value {
    heap.list(vec![sym(kw::QUOTE), v])
}

/// Builder code for one template position. `~x` becomes `x` (evaluated in place
/// by the normal evaluator); a list/vector/map recurses; a literal symbol is
/// quoted (after auto-gensym rewriting `x#`); a self-evaluating atom is emitted
/// verbatim.
///
/// `level` is the quasiquote level (ADR-503): 1 in the template being expanded, one
/// more inside each nested `` ` ``, one less inside each `~`/`~@`. Only a hole that
/// brings it to 0 is evaluated here; anything deeper is rebuilt as written, its
/// `quasiquote`/`unquote` markers included, for the inner template to expand when it
/// runs. `` `(a `(b ~(+ 1 2))) `` is `(a `(b ~(+ 1 2)))`, and with `x` bound to `y`,
/// `` `(a `(b ~~x)) `` is `(a `(b ~y))`.
fn qq_elem(heap: &mut Heap, v: Value, depth: u32, level: u32, autogen: &mut AutoGen) -> LispResult {
    if depth >= MAX_DEPTH {
        return Err(LispError::runtime(format!(
            "quasiquote template nested too deeply (max {} levels)",
            MAX_DEPTH
        )));
    }
    if level == 1 {
        // ~x → x : the unquoted form is evaluated in place when the builder runs.
        if let Some(inner) = tagged(heap, v, kw::UNQUOTE) {
            return Ok(inner);
        }
        // ~@x at a non-sequence position has nothing to splice into — `qq_seq`
        // handles splices inline, so reaching here means a top-level `~@`. Reject it
        // rather than silently mis-building `(list 'unquote-splicing x)`.
        if tagged(heap, v, kw::UNQUOTE_SPLICING).is_some() {
            return Err(LispError::runtime(
                "unquote-splicing (~@) outside a list/vector context",
            ));
        }
    }
    match v.unpack() {
        ValueRef::Pair(_) => {
            let items = heap.list_to_vec(v)?;
            // A marker form changes the level of what it holds; the marker symbol itself
            // is data, quoted like any other.
            let inner_level = match items.first().map(|h| h.unpack()) {
                Some(ValueRef::Sym(h))
                    if items.len() == 2 && value::symbol_is(h, kw::QUASIQUOTE) =>
                {
                    level + 1
                }
                Some(ValueRef::Sym(h))
                    if level > 1
                        && items.len() == 2
                        && (value::symbol_is(h, kw::UNQUOTE)
                            || value::symbol_is(h, kw::UNQUOTE_SPLICING)) =>
                {
                    level - 1
                }
                _ => level,
            };
            if inner_level != level {
                let marker = quote_form(heap, items[0]);
                let held = qq_seq(heap, &items[1..], false, depth + 1, inner_level, autogen)?;
                return Ok(heap.list(vec![sym("cons"), marker, held]));
            }
            qq_seq(heap, &items, false, depth + 1, level, autogen)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            qq_seq(heap, &items, true, depth + 1, level, autogen)
        }
        ValueRef::Map(id) => {
            // No `~@` splicing into a map (ill-defined); expand each key/value.
            let entries = heap.map_entries(id);
            let mut out = Vec::with_capacity(entries.len() * 2 + 1);
            out.push(sym("hash-map"));
            for (k, val) in entries {
                out.push(qq_elem(heap, k, depth + 1, level, autogen)?);
                out.push(qq_elem(heap, val, depth + 1, level, autogen)?);
            }
            Ok(heap.list(out))
        }
        // A set template builds a `(%set e…)` — the set counterpart of the map
        // arm above. Without this arm `#{…}` fell through to the verbatim
        // catch-all, so the *evaluator* evaluated the set's elements instead of
        // the template quoting them: `` `#{a} `` silently gave `#{5}` where
        // `` `(a) ``/`` `[a] `` correctly give the symbol, and `` `#{~x} `` died
        // with "unbound symbol: unquote". No `~@` splicing into a set, same as a
        // map — `qq_elem` rejects a stray `~@` for us.
        ValueRef::Set(id) => {
            let elems = heap.map_entries(id);
            let mut out = Vec::with_capacity(elems.len() + 1);
            out.push(sym("%set"));
            for (e, _) in elems {
                out.push(qq_elem(heap, e, depth + 1, level, autogen)?);
            }
            Ok(heap.list(out))
        }
        // A literal symbol is data — quote it (auto-gensym `x#` first). An `x#` belongs to
        // the template that directly holds it: deeper, it is left for that template.
        ValueRef::Sym(_) => {
            let sv = if level == 1 {
                maybe_autogensym(v, autogen)
            } else {
                v
            };
            Ok(quote_form(heap, sv))
        }
        // Self-evaluating atoms (int/float/string/keyword/bool/nil) emit verbatim.
        other => Ok(other),
    }
}

/// Builder code for a sequence template (`is_vector` chooses list vs vector).
/// With no `~@` splice it is a flat `(list e…)` / `(vector e…)`. With a splice
/// it is `(append (list e) <spliced-seq> …)`, and for a vector that assembled
/// list is turned back into a vector with `(apply vector …)`. `append` is the
/// seq-generic concatenation, so a spliced vector/list/map flattens uniformly,
/// exactly as the old walker's `seq_items` did.
fn qq_seq(
    heap: &mut Heap,
    items: &[Value],
    is_vector: bool,
    depth: u32,
    level: u32,
    autogen: &mut AutoGen,
) -> LispResult {
    // A dotted unquote tail: `` `(a b . ~t) `` reads as `(a b unquote t)` — the pair
    // `(unquote t)` IS the cdr — so a bare `unquote` symbol second-to-last, with something
    // before it, marks the evaluated tail (built element by element it would come out as
    // the literal `(a b unquote t)`). `~@` there means the same tail (there is nothing to splice
    // it into but the end). The prefix conses onto the tail, so a non-list tail makes the
    // dotted pair the template spells: `` `(1 . ~2) `` is `(1 . 2)`.
    if !is_vector && items.len() >= 3 {
        let marker = items[items.len() - 2];
        if matches!(marker.unpack(), ValueRef::Sym(s)
            if value::symbol_is(s, kw::UNQUOTE) || value::symbol_is(s, kw::UNQUOTE_SPLICING))
        {
            let prefix = &items[..items.len() - 2];
            let tail = items[items.len() - 1];
            if level > 1 {
                // Deeper than the template being expanded: rebuild `(a unquote t)` as
                // written, `t` one level down.
                let mut out = vec![sym("list")];
                for &it in prefix {
                    out.push(qq_elem(heap, it, depth, level, autogen)?);
                }
                out.push(quote_form(heap, marker));
                out.push(qq_elem(heap, tail, depth, level - 1, autogen)?);
                return Ok(heap.list(out));
            }
            let prefix_has_splice = prefix
                .iter()
                .any(|&it| tagged(heap, it, kw::UNQUOTE_SPLICING).is_some());
            if prefix_has_splice {
                let built = qq_seq(heap, prefix, false, depth, level, autogen)?;
                return Ok(heap.list(vec![sym("append"), built, tail]));
            }
            let mut out = tail;
            for &it in prefix.iter().rev() {
                let e = qq_elem(heap, it, depth, level, autogen)?;
                out = heap.list(vec![sym("cons"), e, out]);
            }
            return Ok(out);
        }
    }
    // A `~@` splices only into the template being expanded; deeper it is data.
    let has_splice = level == 1
        && items
            .iter()
            .any(|&it| tagged(heap, it, kw::UNQUOTE_SPLICING).is_some());
    if !has_splice {
        let mut out = Vec::with_capacity(items.len() + 1);
        out.push(if is_vector {
            sym("vector")
        } else {
            sym("list")
        });
        for &it in items {
            out.push(qq_elem(heap, it, depth, level, autogen)?);
        }
        return Ok(heap.list(out));
    }
    let mut segs = Vec::with_capacity(items.len() + 1);
    segs.push(sym("append"));
    for &it in items {
        if let Some(inner) = tagged(heap, it, kw::UNQUOTE_SPLICING) {
            segs.push(inner); // splice the sequence's elements in place
        } else {
            let e = qq_elem(heap, it, depth, level, autogen)?;
            let one = heap.list(vec![sym("list"), e]);
            segs.push(one);
        }
    }
    let appended = heap.list(segs);
    if is_vector {
        Ok(heap.list(vec![sym("apply"), sym("vector"), appended]))
    } else {
        Ok(appended)
    }
}

/// If `v` is a two-element list `(name x)` with the given head symbol, return `x`.
fn tagged(heap: &Heap, v: Value, name: &str) -> Option<Value> {
    if let ValueRef::Pair(p) = v.unpack() {
        let (head, tail) = heap.pair(p);
        if let ValueRef::Sym(s) = head.unpack()
            && value::symbol_is(s, name)
            && let ValueRef::Pair(p2) = tail.unpack()
        {
            return Some(heap.car(p2));
        }
    }
    None
}

/// If `sym` (a combination head, resolved in `env`) names a **macro**, return its
/// closure id. Resolves the head the way the eval-time dispatch and the `resolve`
/// pass do, so a bare `(:use mod)`-imported macro (or a same-namespace `ns/name`
/// macro) is recognised — not only a directly-bound one (ADR-065). Used by
/// `macroexpand_1` to expand, and by the compiling VM (`eval::compile`) to **defer**
/// a closure whose body still contains an unexpanded (forward-referenced) macro
/// call — both must agree on what "is a macro head" means.
pub(crate) fn macro_head_id(heap: &Heap, env: EnvId, sym: value::Symbol) -> Option<ClosureId> {
    match heap.env_get(env, sym).map(|v| v.unpack()) {
        // A ROOT macro whose bare name this module defines or imports: the module's own
        // name wins inside it (ADR-423), exactly as it does for a function — its macro
        // expands, its function is called. `/name` still reaches the root macro, and the
        // prelude's own templates emit `/name` (`template_escape`), so they are not captured.
        Some(ValueRef::Macro(mid)) => match heap.compile_ns() {
            Some(ns) if !value::symbol_name_ref(sym).contains('/') => {
                let own = resolve_sym(heap, sym, value::symbol_name_ref(ns), &[]);
                if own == sym {
                    Some(mid)
                } else {
                    match heap.env_get(value::EnvId::GLOBAL, own).map(|v| v.unpack()) {
                        Some(ValueRef::Macro(own_mid)) => Some(own_mid),
                        _ => None,
                    }
                }
            }
            _ => Some(mid),
        },
        // Directly bound to a non-macro (a local, or a non-macro global): it
        // shadows — never reinterpret it as an imported macro.
        Some(_) => None,
        // Unbound directly: a bare reference that may name an imported /
        // same-namespace macro. Resolve it as the `resolve` pass does.
        None => {
            let q = match heap.compile_ns() {
                Some(ns) => resolve_sym(heap, sym, value::symbol_name_ref(ns), &[]),
                // At ROOT there is no namespace to resolve against, but a `/name` root
                // escape (ADR-236) still has to name the root binding — and a MACRO head
                // has to be recognised here, at expansion time, not merely rewritten
                // later by `resolve`. `compile` expands before it resolves, so an
                // unrecognised `/sig` stays unexpanded and never produces the
                // `%register-sig` the checker collects: every record's field types
                // silently stop being checked. (`/or` and friends appear to work anyway
                // only because the *evaluator* expands macros at runtime — the checker
                // never evaluates, so it does not get that second chance.)
                None if is_root_escape(sym) => value::intern(&value::symbol_name(sym)[1..]),
                None => heap.import_of(sym).unwrap_or(sym),
            };
            // `heap.global()`, not `EnvId::GLOBAL`: during a source boot the prelude's globals
            // live in the builder's frame (see `EnvId::GLOBAL`), and every prelude function
            // whose body uses a prelude macro meets a `/when`-style head here (ADR-423). Asked
            // of the table, it missed, so those heads stayed unexpanded, the closures were
            // deferred to the tree-walker for good, and loading any std module paid for it
            // (+16M instructions, and background JIT work on top).
            match (q != sym, heap.env_get(heap.global(), q).map(|v| v.unpack())) {
                (true, Some(ValueRef::Macro(mid))) => Some(mid),
                _ => None,
            }
        }
    }
}

/// Expand `form` by one step if its head is a macro; returns `(expanded, did_expand)`.
pub fn macroexpand_1(heap: &mut Heap, form: Value, env: EnvId) -> Result<(Value, bool), LispError> {
    let ValueRef::Pair(p) = form.unpack() else {
        return Ok((form, false));
    };
    let ValueRef::Sym(s) = heap.pair(p).0.unpack() else {
        return Ok((form, false));
    };
    // A qualified call head into a not-yet-loaded module may be a macro that
    // must expand at compile time (the compile-time require-for-macros rule) —
    // infer the require from the qualified name so the module loads before this lookup.
    // A no-op for a bare head or an already-loaded one (ADR-227 follow-up).
    let mut form = form;
    let mut env = env;
    // `(def std-module/name …)`: load that module before the `def` runs, so its names are
    // sealed and the reserved check refuses the rebind whatever was loaded before
    // (`derive::require_def_target`). `form` is rooted across the load for the reason given
    // below; `env` is not read after it.
    if value::symbol_is(s, kw::DEF)
        && let Some(ValueRef::Sym(target)) = heap
            .list_items(form)
            .and_then(|items| items.get(1).map(|v| v.unpack()))
    {
        let rb = heap.roots_len();
        heap.push_root(form);
        let r = crate::eval::derive::require_def_target(heap, env, target);
        form = heap.root_at(rb);
        heap.truncate_roots(rb);
        r?;
        return Ok((form, false));
    }
    let mut head_macro = macro_head_id(heap, env, s);
    if head_macro.is_none() {
        // The require LOADS a module — arbitrary eval, therefore a collection that
        // relocates `form` and the env chain. Root both across it and re-read: the
        // `(macroexpand-1 …)` / `(macroexpand …)` BUILTINS reach here with
        // MACRO_BLOCK *off* (unlike the compile pass, which holds a
        // `MacroBlockGuard`), so a handle read before this call and used after it is
        // a live use-after-GC — an epoch-tripwire abort in debug and silent heap
        // corruption in release (it walked relocated memory and reported a garbage
        // list length as an arity error).
        let rb = heap.roots_len();
        let eb = heap.env_roots_len();
        heap.push_root(form);
        heap.push_env_root(env);
        let r = crate::eval::derive::require_qualified_head(heap, env, s);
        form = heap.root_at(rb);
        env = heap.env_root_at(eb);
        heap.truncate_roots(rb);
        heap.truncate_env_roots(eb);
        // Look again only if a module was required: nothing else could bind the head.
        if r? {
            head_macro = macro_head_id(heap, env, s);
        }
    }
    let Some(mid) = head_macro else {
        return Ok((form, false));
    };
    // A check that expands this call depends on the macro (ADR-485): the expansion is
    // what it analyses, and no observation of the expanded tree names the macro. A no-op
    // outside a recording check.
    if let Some(name) = heap.closure(mid).name {
        heap.rec_check_dep_sym(name);
    }
    // Re-derive the tail from the (possibly relocated) form rather than reusing the
    // one read before the require.
    let ValueRef::Pair(p) = form.unpack() else {
        return Ok((form, false));
    };
    let tail = heap.pair(p).1;
    let args = heap.list_to_vec(tail)?;
    // Run the expander through the ACTIVE ENGINE (VM when enabled), not the
    // tree-walker. Paired with the compile pass expanding a macro's
    // autogensym-free `quasiquote` body to builder code (see `compile`), a
    // hot macro (`defn`, …) now compiles once and expands as bytecode/native
    // — macro expansion dominated the advisory checker (ADR-119). Same result
    // as `apply_closure`; the VM is the default engine for all other calls.
    let expanded = crate::eval::compile::apply_engine(heap, Value::Fn(mid), &args, env)?;
    Ok((expanded, true))
}

/// Repeatedly expand `form` until its head is no longer a macro.
pub fn macroexpand(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    // `macroexpand_1` applies the expander, which can collect at ANY depth
    // (ADR-061) — and the `(macroexpand …)` builtin reaches this at runtime with
    // MACRO_BLOCK *off* — so `env` must survive across iterations. Root it and
    // re-read; `cur` is the expander's fresh (current-epoch) result each round, or
    // the initial `form` before any eval, so it needs no slot.
    let eb = heap.env_roots_len();
    heap.push_env_root(env);
    // The original call, rooted. An expander that raises is complaining about the call, so
    // an error with no position of its own is placed there — not at the enclosing top-level
    // form the compile pass would otherwise blame — and that includes an expansion a macro
    // BUILT (`match` → `%match`), which has no reader position.
    let rb = heap.roots_len();
    heap.push_root(form);
    let mut cur = form;
    // Bounded fixpoint (kernel audit): a macro that forever expands to another
    // macro call (`(defmacro m (x) `(m ~x))`) otherwise hard-hangs the expander
    // — mitigated only by green-process preemption, and not at all on a
    // no-deadline root-thread expansion. Same cap as the recursion guards.
    let mut rounds = 0u32;
    loop {
        if rounds >= MAX_EXPAND_ROUNDS {
            heap.truncate_env_roots(eb);
            heap.truncate_roots(rb);
            return Err(LispError::runtime(format!(
                "macro expansion did not reach a fixpoint after {} rounds \
                 (a macro that expands to itself?)",
                MAX_EXPAND_ROUNDS
            )));
        }
        rounds += 1;
        let env_now = heap.env_root_at(eb);
        let (next, expanded) = match macroexpand_1(heap, cur, env_now) {
            Ok(r) => r,
            Err(e) => {
                heap.truncate_env_roots(eb);
                let original = heap.root_at(rb);
                heap.truncate_roots(rb);
                return Err(e.or_form_pos(heap, original));
            }
        };
        if !expanded {
            heap.truncate_env_roots(eb);
            heap.truncate_roots(rb);
            return Ok(next);
        }
        cur = next;
    }
}

/// The compile pass: recursively expand *every* macro call in `form` (a code
/// walk), so the result contains no macro invocations and can be evaluated
/// without expanding again. Run once at each top-level / definition boundary
/// (`eval_str`, `load`, `require`, `eval`, and the prelude loader), so a macro
/// in a function body — notably `match` — is expanded ONCE rather than on every
/// call. The evaluator still expands macros lazily as a fallback, which covers
/// a macro defined and used within the same top-level form (not yet defined
/// when the walk ran).
///
/// `quote` and `quasiquote` are left opaque: their contents are data, not calls
/// to expand. Code inside a `~unquote` still expands when the quasiquote runs.
pub fn macroexpand_all(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    let expanded = macroexpand_all_depth(heap, form, env, 0)?;
    if SOURCE_REWRITES_OFF.with(|c| c.get()) {
        return Ok(expanded);
    }
    // Three source rewrites on the fully expanded form — each decided on the WHOLE form,
    // because each reads names by spelling and must know no binder in the form shadows
    // them, and only the whole form can say so. In order: a `seq/l*` stage chain under a
    // `fold`/`reduce` fuses into one literal (§7); a tally through a `fold`/`reduce` literal
    // builds in place (ADR-360 §6); a `fold`/`reduce` over a literal `(range …)` with a
    // literal `fn` becomes a counted `letrec` loop (§7).
    // All three fire only under a four-item `fold`/`reduce` call: one walk answers "none
    // here" for the three, which is nearly every top-level form.
    if !form_has_fold_call(heap, expanded) || namespace_rebinds_rewrite_names(heap) {
        return Ok(expanded);
    }
    let fused = pipeline_fuse_rewrite(heap, expanded);
    let tallied = linmap_fold_rewrite(heap, fused);
    Ok(range_fold_rewrite(heap, tallied))
}

/// Does the namespace being compiled give one of the names the source rewrites read a meaning
/// of its own — a definition, or a `(:use …)` import? The rewrites run before namespaces
/// resolve and read `fold`, `reduce`, `get`, `+`, … by spelling, so in a module that defines
/// its own `fold` they would replace that module's call with the root fold's loop. Their own
/// emitted names are root escapes (ADR-638) and need no such check; a local binder is caught
/// per form by `form_binds_any`.
fn namespace_rebinds_rewrite_names(heap: &Heap) -> bool {
    /// The names the rewrites READ from user code — the calls they recognise, and the
    /// operators of a tally literal they lower. Everything they emit is a root escape.
    static READ: std::sync::LazyLock<Vec<Symbol>> = std::sync::LazyLock::new(|| {
        [
            "fold", "reduce", "range", "get", "assoc", "+", "-", "inc", "dec",
        ]
        .iter()
        .map(|name| value::intern(name))
        .collect()
    });
    heap.compile_ns().is_some()
        && READ
            .iter()
            .any(|&name| resolve_reference(heap, name) != name)
}

thread_local! {
    static SOURCE_REWRITES_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While alive on this thread, `macroexpand_all` expands without the optimiser's source
/// rewrites (the pipeline fusion, the tally rewrite, the counted range loop). The CHECKER
/// holds one: it checks the author's code, and the rewrites are semantics-preserving, so
/// the original is what its diagnostics and its inference should read — a rewritten
/// `(fold (range 16) {} …)` becomes `(if (range? r) <letrec loop> <fold>)`, whose union the
/// checker types less precisely than the fold alone. The evaluator never holds one.
pub struct NoSourceRewrites {
    was: bool,
}

impl NoSourceRewrites {
    pub fn enter() -> Self {
        let was = SOURCE_REWRITES_OFF.with(|c| c.replace(true));
        NoSourceRewrites { was }
    }
}

impl Drop for NoSourceRewrites {
    fn drop(&mut self) {
        SOURCE_REWRITES_OFF.with(|c| c.set(self.was));
    }
}

/// The inverse, held by the MODULE LOADER (`load`, `%load-module-source`): while alive,
/// `macroexpand_all` applies the rewrites whatever the caller holds. A module's bodies are
/// the runtime's, never "the author's code under check" — but the checker's compile pass
/// holds a [`NoSourceRewrites`] while `compile` itself infers and performs the file's
/// `require`s (ADR-227), and the transitive scan (ADR-340) loads more from inside the same
/// check. Without this every module a `brood file.blsp` pre-flight loads would run
/// UNREWRITTEN for the rest of the program (KI-173) — and a stdlib image written by such a
/// process would carry the unrewritten bodies to every later run.
pub struct SourceRewritesOn {
    was: bool,
}

impl SourceRewritesOn {
    pub fn enter() -> Self {
        let was = SOURCE_REWRITES_OFF.with(|c| c.replace(false));
        SourceRewritesOn { was }
    }
}

impl Drop for SourceRewritesOn {
    fn drop(&mut self) {
        SOURCE_REWRITES_OFF.with(|c| c.set(self.was));
    }
}

fn macroexpand_all_depth(heap: &mut Heap, form: Value, env: EnvId, depth: u32) -> LispResult {
    // Expansion recurses per nesting level of the form; a deeply-nested-but-
    // legal form (30k+ levels) would blow the native stack. Grow it in
    // heap-backed segments instead (the same stacker remedy as the deep-VALUE
    // walkers) — the fast path is a
    // couple of compares, and depth stays structurally bounded by the form.
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        macroexpand_all_depth_inner(heap, form, env, depth)
    })
}

fn macroexpand_all_depth_inner(heap: &mut Heap, form: Value, env: EnvId, depth: u32) -> LispResult {
    let out = macroexpand_all_depth_step(heap, form, env, depth)?;
    // **Synthetic code is located at the form it was expanded from.** Whatever this step
    // produced — a macro's result (built by its body: quasiquote, `list`, `cons`), a
    // pattern-binder desugar, a rebuilt list — its new pairs were never seen by the reader
    // and carry no position, while the sub-forms the user WROTE are the same pair values
    // and keep their exact positions. Stamp the original form's position onto every
    // unpositioned pair of the result. Every consumer of positions is served at once: a
    // checker lint that fires inside a `match`'s expansion reports at the `match` rather
    // than nowhere, and a runtime error raised from generated code has a location.
    // Linear, not quadratic, because the walk stops at a positioned pair: children are
    // expanded before their parent, so a positioned subtree was already finished.
    if out.as_pair() != form.as_pair() {
        // The whole record — the position AND ITS FILE. The stamp also runs at lazy
        // expansion (an arm compiled at first call, `%coverage-precompile`, hot reload),
        // where `current_file` is a different file or none, so the ambient file is wrong
        // by construction there; the file must travel with the position it belongs to.
        if let Some((pos, file)) = heap.form_pos(form) {
            stamp_synthetic(heap, out, pos, file);
        }
    }
    Ok(out)
}

fn macroexpand_all_depth_step(heap: &mut Heap, form: Value, env: EnvId, depth: u32) -> LispResult {
    // Every 32 levels, make sure the next 32 fit: the step recurses once per nesting
    // level and re-enters `eval` for each macro application on the way.
    if depth % 32 == 31 {
        crate::stack::maybe_grow(256 * 1024, 4 * 1024 * 1024, || {
            macroexpand_all_depth_step_inner(heap, form, env, depth)
        })
    } else {
        macroexpand_all_depth_step_inner(heap, form, env, depth)
    }
}

fn macroexpand_all_depth_step_inner(
    heap: &mut Heap,
    form: Value,
    env: EnvId,
    depth: u32,
) -> LispResult {
    // Block GC during the expansion: this walk holds partially-built LOCAL forms
    // in Rust locals and recurses into macro applications via `eval`, whose
    // safepoint would otherwise sweep them. The runtime evaluator roots its
    // transients on the operand stack so its safepoint fires at any depth
    // (ADR-061) — but the compile pass opts out instead: `MacroBlockGuard` keeps
    // `MACRO_BLOCK > 0` for the expansion, and the safepoint skips collection
    // while that holds. Expansion is bounded per form, so memory grows briefly
    // (reclaimed at the next runtime safepoint). The `GcBlockGuard` is kept too,
    // purely for the stack-depth accounting it feeds. See `docs/memory-model.md`.
    if depth >= MAX_DEPTH {
        return Err(LispError::runtime(format!(
            "macro expansion nested too deeply (max {} levels)",
            MAX_DEPTH
        )));
    }
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    let call = form;
    let form = macroexpand(heap, form, env)?;
    // The record a rebuild of this form copies: the expansion's own when it has one — a
    // form the user wrote that the macro returned, or one it located (`%copy-form-pos`) —
    // else the macro call's, as an expansion built from nothing is located at its call.
    // Copying the call's unconditionally put `(w (+ 1 "x"))`'s error on the `w`.
    let original = if form.as_pair() != call.as_pair() && heap.form_pos_only(form).is_some() {
        form
    } else {
        call
    };
    match form.unpack() {
        ValueRef::Pair(_) => {
            let items = match heap.list_items(form) {
                Some(items) => items,
                // An improper list: nothing to expand, but a cell of its own (`copy_form`).
                None => return Ok(copy_form(heap, form)),
            };
            if let Some(ValueRef::Sym(head)) = items.first().copied().map(|v| v.unpack()) {
                // quote/quasiquote contents are data, not calls to expand. The form still
                // gets cells of its own, as every list this walk returns does: a macro that
                // splices one quoted argument into two `fn` bodies would otherwise hand
                // both the same first body form (`copy_form`).
                if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
                    return Ok(copy_form(heap, form));
                }
                // (`fn` is the only spelling — no `lambda` alias, ADR-162.)
                let s = head;
                // Typed parameters (ADR-420): `(def NAME (fn (x:int -> int) …))` — what
                // `defn` expands to — becomes the plain definition plus its declaration,
                // before anything below sees the parameter list.
                if value::symbol_is(s, kw::DEF)
                    && let Some(rewritten) = typed_def(heap, &items)?
                {
                    return macroexpand_all_depth(heap, rewritten, env, depth + 1);
                }
                // Desugar pattern binders into the Brood `%match` engine so they
                // expand once here (fast) rather than per call. eval's `let`/`fn`
                // then only ever see plain symbol binds.
                if value::symbol_is(s, kw::LET) || value::symbol_is(s, kw::LETREC) {
                    refuse_typed_let(
                        heap,
                        if value::symbol_is(s, kw::LET) {
                            "let"
                        } else {
                            "letrec"
                        },
                        &items,
                    )?;
                    // A **vector** bindings container — `(let [a 1] …)`, and with it
                    // the Clojure `(let [[a 1] [b 2]] …)` shape, which would otherwise
                    // destructure `[a 1]` against `[b 2]` and report a bewildering
                    // `unbound symbol: b`. Lists for code (ADR-010): reject it here,
                    // in the pass both engines share.
                    if matches!(items.get(1).map(|v| v.unpack()), Some(ValueRef::Vector(_))) {
                        let who = if value::symbol_is(s, kw::LET) {
                            "let"
                        } else {
                            "letrec"
                        };
                        return Err(crate::eval::vector_binding_container_error(who));
                    }
                }
                if value::symbol_is(s, kw::LET) {
                    // A nested-Scheme binding form with literal values —
                    // `(let ((a 1) (b 2)) …)` — would otherwise lower into a
                    // refutable pattern match on `(b 2)` and die with a confusing
                    // "unbound symbol". Catch it here (the compile pass every engine
                    // shares, so both the VM and tree-walker report it) with the
                    // flatten hint. The literal-value guard keeps a genuine bare-list
                    // destructure like `(let ((a b) '(1 2)) …)` untouched.
                    if let Some(binds) = items.get(1).and_then(|&b| heap.list_or_vector_items(b))
                        && crate::eval::even_bindings_look_scheme(heap, &binds)
                    {
                        return Err(crate::eval::scheme_binding_error(heap, "let", &binds));
                    }
                    if let Some(lowered) = lower_let(heap, &items) {
                        return macroexpand_all_depth(heap, lowered, env, depth + 1);
                    }
                    // Ordinary let: expand binding *values* and the body, but not the
                    // binding *targets* — a bound name must not be expanded as a call.
                    return expand_let(heap, original, &items, env, depth + 1);
                } else if value::symbol_is(s, kw::LETREC) {
                    // Same shape as let: even-indexed binding entries are targets
                    // (opaque), odd-indexed are values (expand). letrec disallows
                    // pattern targets in eval, so there's no `lower_let` branch.
                    return expand_let(heap, original, &items, env, depth + 1);
                } else if value::symbol_is(s, kw::FN) {
                    if let Some(plain) = strip_fn(heap, &items)? {
                        return macroexpand_all_depth(heap, plain, env, depth + 1);
                    }
                    if let Some(lowered) = lower_fn(heap, &items)? {
                        return macroexpand_all_depth(heap, lowered, env, depth + 1);
                    }
                    // `lower_fn` declined: this is either a single-clause fn (its
                    // param list at items[1]) or an arity-only *multi*-clause fn
                    // (each remaining form is a `(param-list body…)` clause, built
                    // into `ClosureArm`s by the evaluator). For multi-clause, expand
                    // each clause's BODY while leaving its param list opaque; for
                    // single-clause, expand only the body after the param list.
                    if fn_is_arity_multi_clause(heap, &items) {
                        return expand_fn_clauses(heap, original, &items, env, depth + 1);
                    }
                    return expand_tail(heap, original, &items, 2, env, depth + 1);
                } else if value::symbol_is(s, kw::DEFMACRO) {
                    if let Some(plain) = strip_defmacro(heap, &items)? {
                        return macroexpand_all_depth(heap, plain, env, depth + 1);
                    }
                    // (defmacro name params body...) — name/params aren't calls.
                    return expand_tail(heap, original, &items, 3, env, depth + 1);
                }
            }
            // `(def NAME (fn …))`: the `fn`'s lowering names its no-match error `NAME`.
            let defined_fn = match (items.first(), items.get(1), items.get(2)) {
                (Some(&head), Some(&name), Some(&value)) if items.len() == 3 => {
                    match (head.unpack(), name.unpack(), heap.list_items(value)) {
                        (ValueRef::Sym(h), ValueRef::Sym(name), Some(value_items))
                            if value::symbol_is(h, kw::DEF)
                                && value_items.first().is_some_and(|v| {
                                    matches!(v.unpack(), ValueRef::Sym(f) if value::symbol_is(f, kw::FN))
                                }) =>
                        {
                            Some(name)
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                if index == 2
                    && let Some(name) = defined_fn
                {
                    defining_fn(name);
                }
                let expanded = macroexpand_all_depth(heap, item, env, depth + 1);
                if index == 2 {
                    end_defining(); // never left for an unrelated `fn`
                }
                out.push(expanded?);
            }
            // Wrapper-split a linear immutable-map fold into an in-place table loop
            // (ADR-112, ADR-360 in docs/decisions.md); applies to a qualifying
            // `(def NAME (fn …))` that passes the `linmap_probe` reachability gate.
            // On by default; opt out with `BROOD_LINMAP=0`.
            // Like the fold rewrites, it reads `get`/`assoc`/`+` by spelling: not in a module
            // that gives one of them its own meaning.
            // (the shape test first: it is cheap, and the namespace check is not)
            let defines_fn = out.len() == 3
                && matches!(out[0].unpack(), ValueRef::Sym(d) if value::symbol_is(d, kw::DEF))
                && heap.list_items(out[2]).is_some_and(|value| {
                    matches!(value.first().map(|v| v.unpack()),
                        Some(ValueRef::Sym(f)) if value::symbol_is(f, kw::FN))
                });
            if defines_fn
                && !namespace_rebinds_rewrite_names(heap)
                && let Some(split) = linmap_split_def(heap, &out)
            {
                return Ok(split);
            }
            Ok(rebuild_list(heap, original, out))
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(macroexpand_all_depth(heap, item, env, depth + 1)?);
            }
            Ok(heap.alloc_vector(out))
        }
        ValueRef::Map(id) => {
            // Walk a map literal's keys and values so macros inside them expand
            // once here. Keep it a literal map (the evaluator canonicalises it).
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = macroexpand_all_depth(heap, k, env, depth + 1)?;
                let v = macroexpand_all_depth(heap, v, env, depth + 1)?;
                pairs.push((k, v));
            }
            Ok(heap.map_from_pairs(pairs))
        }
        ValueRef::Set(id) => {
            // Walk a set literal's elements so macros inside them expand once here.
            // Keep it a literal set (the evaluator evaluates + dedups it).
            let items = heap.set_elems(id);
            let mut out = Vec::with_capacity(items.len());
            for e in items {
                out.push(macroexpand_all_depth(heap, e, env, depth + 1)?);
            }
            Ok(heap.set_from_elems(out))
        }
        other => Ok(other),
    }
}

/// Give every LOCAL pair reachable from `form` that has no source position the position
/// `pos` — the position of the macro call it was expanded from. Pairs that already have
/// one (the user's own sub-forms, or a finished inner expansion) are left exactly as they
/// are and not descended into. An explicit stack, so a large expansion cannot overflow the
/// Rust stack; quoted data inside is stamped too, harmlessly (a position on data is unused).
pub(crate) fn stamp_synthetic(
    heap: &mut Heap,
    form: Value,
    pos: crate::error::Pos,
    file: Option<std::sync::Arc<str>>,
) {
    use std::collections::HashMap;
    // Pass 1 — every unpositioned pair reachable from `form`, parents before children, not
    // descending into a positioned pair (the user's own text, or a finished inner
    // expansion: nothing below it needs a position — which is what keeps the pass linear).
    let mut order: Vec<Value> = Vec::new();
    let mut stack = vec![form];
    while let Some(v) = stack.pop() {
        match v.unpack() {
            ValueRef::Pair(p) => {
                if heap.form_pos_only(v).is_some() && v.as_pair() != form.as_pair() {
                    continue;
                }
                if heap.form_pos_only(v).is_none() {
                    order.push(v);
                }
                let (head, tail) = heap.pair(p);
                stack.push(head);
                stack.push(tail);
            }
            ValueRef::Vector(id) => {
                for e in heap.vector(id).to_vec() {
                    stack.push(e);
                }
            }
            _ => {}
        }
    }
    // Pass 2 — children before parents: a pair DERIVES a position from the first element on
    // its spine that is a form the user wrote (positioned, not synthetic) or a generated
    // form that itself derived one. So a `match` clause's `(if (%eq m :a) (do body) …)`
    // derives the clause's line through the `(do …)` its body sits in.
    type PosFile = (crate::error::Pos, Option<std::sync::Arc<str>>);
    let mut derived: HashMap<u64, PosFile> = HashMap::new();
    for &v in order.iter().rev() {
        let mut cur = v;
        let mut found: Option<PosFile> = None;
        while let ValueRef::Pair(p) = cur.unpack() {
            let (head, tail) = heap.pair(p);
            if let Some(hid) = head.as_pair() {
                let key = pair_key(hid);
                // Derive the position AND ITS FILE from the same record, so a stamp at
                // lazy-expansion time cannot pair one file's line with another's path.
                let hp = if heap.form_pos_only(head).is_some() && !heap.is_synthetic(head) {
                    heap.form_pos(head)
                } else {
                    derived.get(&key).cloned()
                };
                if hp.is_some() {
                    found = hp;
                    break;
                }
            }
            cur = tail;
        }
        if let (Some(d), Some(id)) = (found, v.as_pair()) {
            derived.insert(pair_key(id), d);
        }
    }
    // Pass 3 — parents before children: a pair takes what it derived, else what its
    // nearest positioned ancestor has, else the form the expansion came from.
    let mut inherited: HashMap<u64, PosFile> = HashMap::new();
    for &v in &order {
        let Some(id) = v.as_pair() else { continue };
        let key = pair_key(id);
        let own = derived
            .get(&key)
            .cloned()
            .or_else(|| inherited.get(&key).cloned())
            .unwrap_or_else(|| (pos, file.clone()));
        heap.set_form_pos_in_file(v, own.0, own.1.clone());
        heap.mark_synthetic(v);
        // hand `own` down the spine and to the head, so descendants inherit it
        let mut cur = v;
        while let ValueRef::Pair(p) = cur.unpack() {
            let (head, tail) = heap.pair(p);
            if let Some(h) = head.as_pair() {
                inherited.entry(pair_key(h)).or_insert_with(|| own.clone());
            }
            if let Some(t) = tail.as_pair() {
                inherited.entry(pair_key(t)).or_insert_with(|| own.clone());
            }
            cur = tail;
        }
    }
}

/// A pair's identity for the stamp's bookkeeping: region, index and generation packed.
fn pair_key(id: crate::core::value::PairId) -> u64 {
    ((id.region() as u64) << 62) | ((id.code_gen() as u64) << 40) | (id.index() as u64)
}

/// Rebuild `items` into a fresh list, copying the source position of the
/// `original` pair (if any). The compile pass goes through this on every list
/// it expands, so source positions survive macroexpansion — diagnostics from
/// inside a nested combination still point at the original line, not at the
/// enclosing top-level form's start. No-op for non-LOCAL originals (see
/// [`Heap::form_pos`](crate::core::heap::Heap::form_pos)).
fn rebuild_list(heap: &mut Heap, original: Value, items: Vec<Value>) -> Value {
    let new_list = heap.list(items);
    // The whole record — position AND the synthetic mark — not the position alone: a
    // rebuilt list is the same logical form (ADR-297).
    heap.copy_form_pos(original, new_list);
    new_list
}

/// A fresh copy of the code tree `form`: every pair new, each carrying its original's form
/// record (position and synthetic mark), every vector, map and set literal rebuilt. A
/// `(quote X)` gets new cells of its own while `X`, the datum, is shared — data has no
/// position and is never compiled, and its identity is what the quote evaluates to.
///
/// **One pair, one position.** The compiling VM keys a compiled closure on the identity of
/// its first body form (`VmCacheKey::Body`), and `env_chain_names` bakes the enclosing
/// names into it on the assumption that every instance of one `fn` form is built in one
/// lexical scope. So a pair placed in two code positions is one closure compiled once for
/// both — and when the two positions are different closures (a rewrite's fast copy and
/// its fallback sharing a leading `'tag`), each runs the other's body. Every pass that
/// places a sub-form in more than one position copies it through here.
fn copy_form(heap: &mut Heap, form: Value) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || copy_form_grown(heap, form))
}

fn copy_form_grown(heap: &mut Heap, form: Value) -> Value {
    match form.unpack() {
        ValueRef::Pair(_) => {
            let mut items = Vec::new();
            let mut cursor = form;
            while let ValueRef::Pair(p) = cursor.unpack() {
                let (head, tail) = heap.pair(p);
                items.push(head);
                cursor = tail;
            }
            let quoted = matches!(items.first().map(|v| v.unpack()),
                Some(ValueRef::Sym(s)) if value::symbol_is(s, kw::QUOTE));
            if !quoted {
                for item in items.iter_mut() {
                    *item = copy_form(heap, *item);
                }
            }
            // `cursor` is the tail past the last pair: nil, or a dotted atom.
            let copied = heap.list_with_tail(items, cursor);
            heap.copy_form_pos(form, copied);
            copied
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let copied: Vec<Value> = items
                .into_iter()
                .map(|item| copy_form(heap, item))
                .collect();
            heap.alloc_vector(copied)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let copied: Vec<(Value, Value)> = entries
                .into_iter()
                .map(|(key, entry)| (copy_form(heap, key), copy_form(heap, entry)))
                .collect();
            heap.map_from_pairs(copied)
        }
        ValueRef::Set(id) => {
            let elements = heap.set_elems(id);
            let copied: Vec<Value> = elements
                .into_iter()
                .map(|element| copy_form(heap, element))
                .collect();
            heap.set_from_elems(copied)
        }
        _ => form,
    }
}

/// Rebuild a form expanding only `items[start..]` (the call's body/argument tail),
/// leaving `items[..start]` opaque. Used to skip binding positions — a fn/defmacro
/// parameter list — so a name there is never mistaken for a macro call.
fn expand_tail(
    heap: &mut Heap,
    original: Value,
    items: &[Value],
    start: usize,
    env: EnvId,
    depth: u32,
) -> LispResult {
    let start = start.min(items.len());
    // Opaque, but copied: a binding position is still one position (`copy_form`).
    let mut out: Vec<Value> = items[..start]
        .iter()
        .map(|&item| copy_form(heap, item))
        .collect();
    for &item in &items[start..] {
        out.push(macroexpand_all_depth(heap, item, env, depth)?);
    }
    Ok(rebuild_list(heap, original, out))
}

/// Does this (post-`lower_fn`) `fn`/`lambda` form's body consist entirely of
/// `(param-list body…)` clauses — i.e. is it an arity-only multi-clause fn? (A
/// leading docstring is allowed.) Pattern multi-clause fns were already lowered
/// to `%match`, so by here "all clauses" implies arity-only.
pub(crate) fn fn_is_arity_multi_clause(heap: &Heap, items: &[Value]) -> bool {
    let forms = &items[1..];
    let forms = match forms.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    !forms.is_empty() && forms.iter().all(|&f| is_clause(heap, f))
}

/// Expand an arity-only multi-clause `fn`: each clause is `(param-list body…)`.
/// Leave each clause's param list opaque (a binding position — a name there must
/// not be expanded as a call) and macroexpand each clause's body forms. A leading
/// docstring is passed through untouched.
fn expand_fn_clauses(
    heap: &mut Heap,
    original: Value,
    items: &[Value],
    env: EnvId,
    depth: u32,
) -> LispResult {
    let mut out = vec![items[0]]; // the `fn`/`lambda` head
    let mut i = 1;
    if matches!(items.get(1).map(|v| v.unpack()), Some(ValueRef::Str(_))) && items.len() > 2 {
        out.push(items[1]); // leading docstring
        i = 2;
    }
    for &clause in &items[i..] {
        match heap.list_or_vector_items(clause) {
            Some(parts) if !parts.is_empty() => {
                let mut co = vec![copy_form(heap, parts[0])]; // param list: opaque, copied
                for &b in &parts[1..] {
                    co.push(macroexpand_all_depth(heap, b, env, depth)?);
                }
                out.push(rebuild_list(heap, clause, co));
            }
            _ => out.push(clause),
        }
    }
    Ok(rebuild_list(heap, original, out))
}

/// Expand an ordinary `let`: its binding *values* (odd positions of the binding
/// list) and its body, leaving the binding *targets* (even positions) opaque.
fn expand_let(
    heap: &mut Heap,
    original: Value,
    items: &[Value],
    env: EnvId,
    depth: u32,
) -> LispResult {
    let Some(bindings) = items.get(1).copied() else {
        return Ok(rebuild_list(heap, original, items.to_vec()));
    };
    let new_bindings = match heap.list_or_vector_items(bindings) {
        Some(binds) => {
            let mut nb = Vec::with_capacity(binds.len());
            for (i, &x) in binds.iter().enumerate() {
                // odd index = a value expression (expand); even = a target (opaque)
                nb.push(if i % 2 == 1 {
                    macroexpand_all_depth(heap, x, env, depth)?
                } else {
                    copy_form(heap, x)
                });
            }
            match bindings.unpack() {
                ValueRef::Vector(_) => heap.alloc_vector(nb),
                _ => rebuild_list(heap, bindings, nb),
            }
        }
        None => bindings,
    };
    let mut out = vec![items[0], new_bindings];
    for &item in &items[2..] {
        out.push(macroexpand_all_depth(heap, item, env, depth)?);
    }
    Ok(rebuild_list(heap, original, out))
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::Interp;
    use crate::syntax::reader;

    /// Resolve `form_src` in namespace `ns`, after evaluating each `defs` line to
    /// set up globals. Returns the printed resolved form.
    fn resolved(defs: &[&str], ns: &str, form_src: &str) -> String {
        let mut interp = Interp::new();
        for d in defs {
            interp.eval_str(d).expect("setup def");
        }
        let nssym = value::intern(ns);
        interp.heap.set_compile_ns(Some(nssym));
        let form = reader::read_one(&mut interp.heap, form_src).expect("parse");
        let out = resolve(&mut interp.heap, form);
        crate::syntax::printer::print(&interp.heap, out)
    }

    #[test]
    fn free_ref_qualifies_when_ns_global_exists() {
        assert_eq!(resolved(&["(def foo/bar 1)"], "foo", "(bar)"), "(foo/bar)");
    }

    #[test]
    fn unknown_free_ref_stays_bare() {
        // `baz` is neither an existing `foo/baz` global nor pre-scanned — root
        // fall-through, left bare (would be unbound at worst, never miscompiled).
        assert_eq!(resolved(&[], "foo", "(baz)"), "(baz)");
    }

    #[test]
    fn root_prelude_name_stays_bare() {
        // `map` is a prelude global; there is no `foo/map`, so it stays root.
        assert_eq!(resolved(&[], "foo", "(map xs f)"), "(map xs f)");
    }

    #[test]
    fn definition_head_is_qualified() {
        assert_eq!(resolved(&[], "foo", "(def bar 1)"), "(def foo/bar 1)");
    }

    /// The bare def-heads scan_regions assigns to module `m`, sorted.
    fn region(regions: &HashMap<Symbol, HashSet<Symbol>>, m: &str) -> Vec<String> {
        let mut v: Vec<String> = regions
            .get(&value::intern(m))
            .map(|s| s.iter().map(|&n| value::symbol_name(n)).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    fn regions_of(
        interp: &Interp,
        forms: &[Value],
    ) -> HashMap<value::Symbol, HashSet<value::Symbol>> {
        scan_regions(&interp.heap, forms)
    }

    #[test]
    fn scan_regions_partitions_defs_by_module_boundary() {
        // ADR-223: two modules in one file — each def head belongs to its own region.
        let mut interp = Interp::new();
        let forms = reader::read_all(
            &mut interp.heap,
            "(defmodule a) (defn x () 1) (defn y () 2) (defmodule b) (defn z () 3)",
        )
        .expect("parse");
        let regions = scan_regions(&interp.heap, &forms);
        assert_eq!(region(&regions, "a"), vec!["x", "y"]);
        assert_eq!(region(&regions, "b"), vec!["z"]);
    }

    #[test]
    fn scan_regions_excludes_pre_module_defs() {
        // A def before the first defmodule is a ROOT def — in no module's region.
        let mut interp = Interp::new();
        let forms = reader::read_all(
            &mut interp.heap,
            "(defn pre () 0) (defmodule a) (defn x () 1)",
        )
        .expect("parse");
        let regions = scan_regions(&interp.heap, &forms);
        assert_eq!(region(&regions, "a"), vec!["x"]); // `pre` is root, not a's

        // scan_def_names is the union of regions — also excludes the pre-module def.
        let flat = scan_def_names(&interp.heap, &forms);
        assert!(flat.contains(&value::intern("x")));
        assert!(!flat.contains(&value::intern("pre")));
    }

    #[test]
    fn scan_regions_collects_names_a_definition_macro_binds() {
        // KI-118. The pre-scan reads UNEXPANDED forms, so a name bound by a MACRO —
        // std's own `defonce`/`defrecord`/`defmulti`, or hatch's `defhtml` — must be in the
        // region's known names, or a reference from a function written ABOVE the
        // definition fails to qualify to `ns/name` and is UNBOUND at runtime.
        let mut interp = Interp::new();
        let forms = reader::read_all(
            &mut interp.heap,
            "(defmodule a) (defonce cache 1) (defhtml card (p) p) (web/page/defhtml two (p) p)",
        )
        .expect("parse");
        assert_eq!(
            region(&regions_of(&interp, &forms), "a"),
            vec!["cache", "card", "two"]
        );
    }

    #[test]
    fn scan_regions_ignores_a_non_def_head_and_a_nested_defmodule() {
        // The rule is the `def…` spelling, not "second element is a symbol": an ordinary
        // call must not donate its argument to the namespace, and a `defmodule` names a
        // MODULE, never a module-level binding.
        let mut interp = Interp::new();
        let forms = reader::read_all(
            &mut interp.heap,
            "(defmodule a) (register thing) (do (defmodule b)) (defn x () 1)",
        )
        .expect("parse");
        assert_eq!(region(&regions_of(&interp, &forms), "a"), vec!["x"]);
    }

    #[test]
    fn a_macro_defined_name_resolves_from_above_its_definition() {
        // The end-to-end shape of KI-118: `read-it` is written above the `defthing` that
        // binds `thing`; it must not be `unbound symbol: thing` at CALL time.
        let mut interp = Interp::new();
        interp
            .eval_str(
                "(defmodule ki118)\n\
                 (defmacro defthing (name val) (list 'def name val))\n\
                 (defn read-it () (* thing 2))\n\
                 (defthing thing 21)",
            )
            .expect("load");
        assert_eq!(
            interp.eval_str("(ki118/read-it)").expect("call").as_int(),
            Some(42)
        );
    }

    #[test]
    fn local_binding_shadows_and_is_not_qualified() {
        // `foo/x` exists, but the `let`-bound `x` is local — must NOT qualify.
        assert_eq!(
            resolved(&["(def foo/x 1)"], "foo", "(let (x 1) x)"),
            "(let (x 1) x)"
        );
    }

    #[test]
    fn fn_param_is_not_qualified_but_free_body_ref_is() {
        // `x` is a param (local); `bar` is a free ref to a ns global → qualified.
        assert_eq!(
            resolved(&["(def foo/bar 1)"], "foo", "(fn (x) (bar x))"),
            "(fn (x) (foo/bar x))"
        );
    }

    #[test]
    fn quoted_symbol_is_never_qualified() {
        // Data: even though `foo/bar` exists, a quoted `bar` is untouched (ADR-034).
        assert_eq!(
            resolved(&["(def foo/bar 1)"], "foo", "(quote bar)"),
            "(quote bar)"
        );
    }

    #[test]
    fn already_qualified_symbol_passes_through() {
        assert_eq!(
            resolved(&["(def other/bar 1)"], "foo", "(other/bar)"),
            "(other/bar)"
        );
    }

    #[test]
    fn imported_macro_expands_in_the_compile_walk() {
        // The `defserver` checker bug (ADR-065): a `(:use mod)`-imported macro must
        // expand during macroexpand/compile, not only a directly-bound one. Without
        // this, the compile pass (and the advisory checker) walks the macro's raw
        // body and flags its clause keywords / pattern vars as unbound.
        let mut interp = Interp::new();
        interp
            .eval_str("(defmacro m/double (x) (list (quote +) x x))")
            .unwrap();
        // Simulate a file that did `(defmodule u (:use m))`: compile in `u` with
        // `double` imported as `m/double`.
        interp
            .heap
            .add_import(value::intern("double"), value::intern("m/double"));
        interp.heap.set_compile_ns(Some(value::intern("u")));
        let g = interp.heap.global();
        let form = reader::read_one(&mut interp.heap, "(double 5)").unwrap();
        let out = macroexpand(&mut interp.heap, form, g).unwrap();
        assert_eq!(crate::syntax::printer::print(&interp.heap, out), "(+ 5 5)");
    }

    #[test]
    fn bare_unimported_macro_is_left_unexpanded() {
        // No `(:use)` import and not directly bound → resolution is positive-evidence
        // only, so the bare head stays a raw call (never a false expansion).
        let mut interp = Interp::new();
        interp
            .eval_str("(defmacro m/double (x) (list (quote +) x x))")
            .unwrap();
        interp.heap.set_compile_ns(Some(value::intern("u")));
        let g = interp.heap.global();
        let form = reader::read_one(&mut interp.heap, "(double 5)").unwrap();
        let out = macroexpand(&mut interp.heap, form, g).unwrap();
        assert_eq!(
            crate::syntax::printer::print(&interp.heap, out),
            "(double 5)"
        );
    }

    #[test]
    fn root_namespace_is_identity() {
        // No `(ns …)` active → compile_ns is None → resolve is a no-op even for a
        // name that would otherwise look qualifiable.
        let mut interp = Interp::new();
        interp.eval_str("(def foo/bar 1)").unwrap();
        // compile_ns left as None (root)
        let form = reader::read_one(&mut interp.heap, "(bar)").unwrap();
        let out = resolve(&mut interp.heap, form);
        assert_eq!(crate::syntax::printer::print(&interp.heap, out), "(bar)");
    }

    #[test]
    fn letrec_binders_visible_in_every_rhs() {
        // Mutually-referenced letrec names are local, never qualified, even with a
        // same-named ns global present.
        // (printer renders an empty param list `()` as `nil`)
        assert_eq!(
            resolved(
                &["(def foo/a 9)"],
                "foo",
                "(letrec (a (fn () (b)) b (fn () (a))) (a))"
            ),
            "(letrec (a (fn nil (b)) b (fn nil (a))) (a))"
        );
    }

    #[test]
    fn quasiquote_template_free_refs_qualify_to_defining_ns() {
        // α: a macro template's free ref to a same-namespace name is frozen
        // qualified at definition time; a prelude name (`map`) stays bare; the
        // macro param (`x`, unquoted) stays bare.
        assert_eq!(
            resolved(
                &["(def foo/helper 1)"],
                "foo",
                "(defmacro m (x) `(helper (map ~x)))"
            ),
            "(defmacro foo/m (x) (quasiquote (foo/helper (map (unquote x)))))"
        );
    }

    #[test]
    fn quasiquote_autogensym_and_quoted_stay_bare() {
        // A `#` auto-gensym binder and a quoted symbol inside a template are left
        // bare (not qualified), even with same-named ns globals present.
        assert_eq!(
            resolved(
                &["(def foo/tmp 1)", "(def foo/k 2)"],
                "foo",
                "(defmacro m () `(let (tmp# 1) (quote k)))"
            ),
            "(defmacro foo/m nil (quasiquote (let (tmp# 1) (quote k))))"
        );
    }

    #[test]
    fn match_pattern_binders_are_not_qualified() {
        // The `%match` clause pattern binds `n`; the body ref to `n` must stay local
        // even though `foo/n` exists, while a free `bar` qualifies.
        assert_eq!(
            resolved(
                &["(def foo/n 1)", "(def foo/bar 2)"],
                "foo",
                "(%match :match v (n (bar n)))"
            ),
            "(%match :match v (n (foo/bar n)))"
        );
    }
}
