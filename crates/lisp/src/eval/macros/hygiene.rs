//! Automatic binding hygiene (ADR-066 amendment, ADR-500): a quasiquote template's own
//! binders are alpha-renamed before the template is built.

use super::*;

// ============================================================================
// Automatic binding hygiene (ADR-066 amendment — "Option A"; ADR-500)
// ============================================================================
//
// A quasiquote template's OWN lexical binders — the names it introduces as *literal*
// symbols — are alpha-renamed to fresh gensyms before the template is built, so a template
// binder can neither capture nor be captured by caller code spliced in via `~`/`~@`.
// Hygiene is thus the DEFAULT: a macro needs no `x#` or `(gensym)` for a safe binding.
//
// - Free-reference hygiene (a template's `helper`/`map` resolving to the *defining*
//   namespace) is handled by the auto-qualifying resolver (ADR-065 §7); this closes the
//   other half — *introduced-binding* capture (ADR-066 concern #2) — without the
//   per-symbol lexical context (fat `Value::Sym`) full Scheme hygiene needs, which ADR-066
//   rejected on ship-by-name/homoiconicity/GC grounds.
// - Scope-aware: a binder renames only the references it actually binds, so a same-named
//   prelude reference elsewhere in the template is untouched. Hence a scope map, not
//   `#`'s flat name→gensym table.
// - Fresh per expansion: a template that binds anything takes the runtime expand path
//   (like `#`), so two nested expansions of one macro — `(m (m x))` — get distinct
//   binders. `template_introduces_binder` gates the static-quasiquote optimisation.
// - Opt-out for intentional anaphora (a name the template deliberately exposes to the
//   caller, e.g. `it` in an `aif`): write `~'it` — an unquoted quoted symbol, which lands
//   in a `~unquote` hole the rename never descends, emitting a literal `it`.
//
// The pass runs on the template BEFORE the macros inside it expand, so it must know where
// each form binds. It knows:
// - the core binders `let`/`letrec`/`fn`, whose binders are PATTERNS (the one pattern
//   grammar, `std/prelude/match.blsp`): a symbol binds; a pin's expression, a quoted
//   symbol, a literal and a `record` name do not; a name repeated inside one pattern (an
//   equality constraint, or each `or` alternative) keeps one replacement. `{:keys [a]}` is
//   left literal: its names ARE the keys looked up, so a renamed `a` would read `:a__12`.
// - the standard library's binding macros, by shape (`binding_shape`): binding pairs
//   (`when-let`, `for`, `doseq`, `with`, …), pattern clauses (`match`, `%match`,
//   `receive`), `try`'s `catch`, `as->`, and the params of `defn`/`defmethod`/`impl` — a
//   renamed param still reads as written wherever a parameter list is shown
//   (`value::source_symbol`).
// - a typed binder `x:int` renames its name and keeps its annotation: `x__12:int`.
//
// Still literal, opt into `#`/`(gensym)` there: a binder that a USER's binding macro
// introduces (the pass cannot know its shape), and a computed binder (`(fn ~params …)`,
// `(let ~bindings …)`), which the macro author builds and so names.

/// A hygiene scope frame: original binder symbol → its replacement in the output.
/// A replacement equal to the original means "bound here, but not renamed" (`_`, a
/// `&`-marker, `x#`, a qualified name, a `:keys` name) — it still SHADOWS an outer rename
/// so an inner reference stays literal.
type HygScope = Vec<(value::Symbol, value::Symbol)>;

/// Look up `s` (innermost — last — binding wins).
fn hyg_lookup(scope: &[(value::Symbol, value::Symbol)], s: value::Symbol) -> Option<value::Symbol> {
    scope
        .iter()
        .rev()
        .find(|(orig, _)| *orig == s)
        .map(|(_, t)| *t)
}

/// A plain binder we rename: not `_`, not a `&`-marker, not `#`-suffixed
/// (auto-gensym owns those), not already qualified.
fn hyg_renamable(s: value::Symbol) -> bool {
    let n = value::symbol_name_ref(s);
    !(n == "_" || n.starts_with('&') || n.contains('/') || (n.len() > 1 && n.ends_with('#')))
}

/// A fresh replacement symbol for a binder named like `orig`.
fn hyg_fresh(orig: value::Symbol) -> value::Symbol {
    let name = value::symbol_name(orig); // owned, so gensym can re-enter the interner
    match value::gensym(&name).unpack() {
        ValueRef::Sym(g) => g,
        _ => orig, // gensym always yields a Sym
    }
}

/// True if `v` is `(unquote …)` / `(unquote-splicing …)` — a computed hole whose
/// binders (`(let ~bindings …)` / `(fn ~params …)`) aren't literal template
/// structure, so we can't rename them.
fn hyg_is_unquote(heap: &Heap, v: Value) -> bool {
    tagged(heap, v, kw::UNQUOTE).is_some() || tagged(heap, v, kw::UNQUOTE_SPLICING).is_some()
}

/// The head symbol of `v`'s list, if it has one.
fn head_sym(heap: &Heap, v: Value) -> Option<value::Symbol> {
    match heap.list_items(v)?.first()?.unpack() {
        ValueRef::Sym(h) => Some(h),
        _ => None,
    }
}

/// The hygiene pre-pass: alpha-rename a template's introduced binders. A no-op
/// (returning the template unchanged, no allocation) when it introduces none — the
/// common static case. GC-blocked otherwise because it builds a parallel template
/// tree, exactly like `resolve`.
pub(super) fn hygiene_rename(heap: &mut Heap, template: Value) -> Value {
    if !template_introduces_binder(heap, template) {
        return template;
    }
    let _gc = crate::process::GcBlockGuard::enter();
    let _macro = crate::process::MacroBlockGuard::enter();
    hyg_walk(heap, template, &[])
}

fn hyg_walk(heap: &mut Heap, v: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || hyg_walk_inner(heap, v, scope))
}

fn hyg_walk_inner(heap: &mut Heap, v: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    match v.unpack() {
        ValueRef::Sym(s) => match hyg_lookup(scope, s) {
            Some(target) => Value::symbol(target),
            None => v,
        },
        ValueRef::Pair(_) => hyg_list(heap, v, scope),
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(hyg_walk(heap, it, scope));
            }
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, val) in entries {
                let k = hyg_walk(heap, k, scope);
                let val = hyg_walk(heap, val, scope);
                pairs.push((k, val));
            }
            heap.map_from_pairs(pairs)
        }
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let mut out = Vec::with_capacity(items.len());
            for e in items {
                out.push(hyg_walk(heap, e, scope));
            }
            heap.set_from_elems(out)
        }
        _ => v,
    }
}

/// A template nested in the one being renamed, at quasiquote `level` (2 just inside it). Its
/// own symbols belong to the code the outer template builds, and are renamed when THAT code
/// expands it (ADR-503), so they are left as written. A hole that brings the level back to 1
/// is the outer template's own code again: `` `(let (x 1) `(f ~x)) `` — the inner `~x`
/// reads the `let`'s `x`, so it takes the same replacement.
fn hyg_nested(
    heap: &mut Heap,
    v: Value,
    scope: &[(value::Symbol, value::Symbol)],
    level: u32,
) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match v.unpack() {
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(v) else {
                return v; // improper list — leave verbatim
            };
            let inner_level = match items.first().map(|h| h.unpack()) {
                Some(ValueRef::Sym(h))
                    if items.len() == 2 && value::symbol_is(h, kw::QUASIQUOTE) =>
                {
                    level + 1
                }
                Some(ValueRef::Sym(_)) if items.len() == 2 && hyg_is_unquote(heap, v) => level - 1,
                _ => level,
            };
            if inner_level == 1 {
                let held = hyg_walk(heap, items[1], scope);
                return rebuild_list(heap, v, vec![items[0], held]);
            }
            if inner_level != level {
                let held = hyg_nested(heap, items[1], scope, inner_level);
                return rebuild_list(heap, v, vec![items[0], held]);
            }
            let out = items
                .iter()
                .map(|&it| hyg_nested(heap, it, scope, level))
                .collect();
            rebuild_list(heap, v, out)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out = items
                .into_iter()
                .map(|it| hyg_nested(heap, it, scope, level))
                .collect();
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let pairs = entries
                .into_iter()
                .map(|(k, val)| {
                    (
                        hyg_nested(heap, k, scope, level),
                        hyg_nested(heap, val, scope, level),
                    )
                })
                .collect();
            heap.map_from_pairs(pairs)
        }
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let out = items
                .into_iter()
                .map(|e| hyg_nested(heap, e, scope, level))
                .collect();
            heap.set_from_elems(out)
        }
        _ => v,
    })
}

/// How a standard-library binding macro binds, by its bare head name. The pass runs before
/// the macros in a template expand, so this is the only way it can see their binders.
#[derive(Clone, Copy)]
enum Shape {
    /// `(head (pattern expr … :keyword expr …) body…)` — sequential pairs, each pattern in
    /// scope for the later pairs and the body; a keyword modifier (`:when`, `:into`) takes
    /// an expression.
    Pairs,
    /// `(head (pattern expr) then else…)` — as `Pairs`, but only `then` sees the binding.
    PairsThenElse,
    /// `(head x… (pattern [:when guard] body…)…)` — the first `n` items (head included)
    /// are not clauses; a `receive` `(after ms body…)` clause binds nothing.
    Clauses(usize),
    /// `(try body… (catch e handler…) (finally …))`.
    Try,
    /// `(as-> expr name forms…)`.
    AsArrow,
    /// `(defn name doc? params body…)` / `(defn name doc? (params body…)…)`.
    Defn,
    /// `(defmethod name ids params body…)`.
    Defmethod,
    /// `(impl ability key (op params body…)…)`.
    Impl,
}

fn binding_shape(head: value::Symbol) -> Option<Shape> {
    let name = value::symbol_name_ref(head);
    match name.strip_prefix('/').unwrap_or(name) {
        "when-let" | "when-some" | "for" | "fold-for" | "doseq" | "with" => Some(Shape::Pairs),
        "if-let" | "if-some" => Some(Shape::PairsThenElse),
        "match" => Some(Shape::Clauses(2)),
        "%match" => Some(Shape::Clauses(3)),
        "receive" => Some(Shape::Clauses(1)),
        "try" => Some(Shape::Try),
        "as->" => Some(Shape::AsArrow),
        "defn" | "defn-" => Some(Shape::Defn),
        "defmethod" => Some(Shape::Defmethod),
        "impl" => Some(Shape::Impl),
        _ => None,
    }
}

fn hyg_list(heap: &mut Heap, form: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    let items = match heap.list_items(form) {
        Some(i) => i,
        None => return form, // improper list — leave verbatim
    };
    if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
        // Data / holes — never descend for renaming: `quote` is data;
        // `unquote`/`unquote-splicing` are caller code resolved in the caller's
        // scope (also where `~'it` — the anaphora opt-out — lands, emitting a
        // literal `it`).
        if value::symbol_is(h, kw::QUOTE)
            || value::symbol_is(h, kw::UNQUOTE)
            || value::symbol_is(h, kw::UNQUOTE_SPLICING)
        {
            return form;
        }
        // A nested template: only its holes back to this level are renamed.
        if value::symbol_is(h, kw::QUASIQUOTE) && items.len() == 2 {
            let held = hyg_nested(heap, items[1], scope, 2);
            return rebuild_list(heap, form, vec![items[0], held]);
        }
        if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
            return hyg_let(heap, form, &items, scope);
        }
        if value::symbol_is(h, kw::FN) {
            let mut out = vec![items[0]];
            out.extend(hyg_fn_parts(heap, &items[1..], scope));
            return rebuild_list(heap, form, out);
        }
        if let Some(shape) = binding_shape(h) {
            return hyg_shaped(heap, form, &items, scope, shape);
        }
    }
    // Generic: rename references in every position (head included) under the same
    // scope. A nested binder that matches an enclosing rename stays consistent
    // (shadowing preserved); one this pass cannot see stays literal.
    hyg_generic(heap, form, &items, scope)
}

fn hyg_generic(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let mut out = Vec::with_capacity(items.len());
    for &it in items {
        out.push(hyg_walk(heap, it, scope));
    }
    rebuild_list(heap, form, out)
}

// ---- patterns ------------------------------------------------------------------------

/// Bind one binder symbol into `scope` and return the symbol to emit in its place. A name
/// already bound since `start` — earlier in the same pattern — keeps its replacement, so
/// an equality constraint (`[x x]`) and `or`'s alternatives stay one name. A typed binder
/// `x:int` renames `x` and keeps `:int`.
fn hyg_bind_sym(s: value::Symbol, scope: &mut HygScope, start: usize) -> Value {
    let name = value::symbol_name_ref(s);
    let (base, annotation) = match name.find(':') {
        Some(i) if i > 0 => (value::intern(&name[..i]), &name[i..]),
        _ => (s, ""),
    };
    if !hyg_renamable(base) {
        scope.push((base, base));
        return Value::symbol(s);
    }
    let target = match scope[start..].iter().rev().find(|(orig, _)| *orig == base) {
        Some((_, target)) => *target,
        None => {
            let fresh = hyg_fresh(base);
            scope.push((base, fresh));
            fresh
        }
    };
    if annotation.is_empty() {
        Value::symbol(target)
    } else {
        let typed = format!("{}{annotation}", value::symbol_name_ref(target));
        Value::symbol(value::intern(&typed))
    }
}

/// Rename the binders of `pattern`, adding them to `scope` (this pattern's own entries
/// begin at `start`). A reference inside the pattern — a pin's expression, a dynamic
/// `bytes` size, an `:or` default — resolves through `scope` as it stands.
fn hyg_pattern(heap: &mut Heap, pattern: Value, scope: &mut HygScope, start: usize) -> Value {
    match pattern.unpack() {
        ValueRef::Sym(s) => hyg_bind_sym(s, scope, start),
        ValueRef::Vector(id) => {
            let elems = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(hyg_pattern(heap, e, scope, start));
            }
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => hyg_map_pattern(heap, id, scope, start),
        ValueRef::Pair(_) => hyg_list_pattern(heap, pattern, scope, start),
        _ => pattern, // a literal
    }
}

fn hyg_list_pattern(heap: &mut Heap, pattern: Value, scope: &mut HygScope, start: usize) -> Value {
    if hyg_is_unquote(heap, pattern) {
        return pattern; // a computed sub-pattern — not visible
    }
    let items = match heap.list_items(pattern) {
        Some(i) => i,
        None => return pattern,
    };
    let head = match items.first().map(|v| v.unpack()) {
        Some(ValueRef::Sym(h)) => Some(h),
        _ => None,
    };
    let mut out = Vec::with_capacity(items.len());
    match head {
        Some(h) if value::symbol_is(h, kw::QUOTE) => return pattern, // the literal symbol
        Some(h) if value::symbol_is(h, kw::PIN) => {
            // `^expr` — matched against the CURRENT value of `expr`: a reference.
            out.push(items[0]);
            for &e in &items[1..] {
                out.push(hyg_walk(heap, e, scope));
            }
        }
        Some(h) if value::symbol_is(h, kw::OR) || value::symbol_is(h, kw::AND) => {
            out.push(items[0]);
            for &p in &items[1..] {
                out.push(hyg_pattern(heap, p, scope, start));
            }
        }
        Some(h) if value::symbol_name_ref(h) == "record" => {
            // `(record name {…})` — the name is a record id, then a map pattern.
            out.extend(items.iter().take(2).copied());
            for &p in items.get(2..).unwrap_or(&[]) {
                out.push(hyg_pattern(heap, p, scope, start));
            }
        }
        Some(h) if value::symbol_name_ref(h) == "bytes" => {
            out.push(items[0]);
            for &segment in &items[1..] {
                out.push(hyg_bytes_segment(heap, segment, scope, start));
            }
        }
        _ => {
            // A list pattern: every element, the first included, is a pattern — except after
            // `&optional`, where a `(pattern default)` slot's default is an expression that
            // reads the names bound before it (ADR-619).
            let mut optional = false;
            for &p in &items {
                match p.unpack() {
                    ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => {
                        optional = true;
                        out.push(p);
                    }
                    ValueRef::Sym(s) if value::symbol_is(s, kw::AMP) => {
                        optional = false;
                        out.push(p);
                    }
                    _ if optional => out.push(hyg_optional_slot(heap, p, scope, start)),
                    _ => out.push(hyg_pattern(heap, p, scope, start)),
                }
            }
        }
    }
    rebuild_list(heap, pattern, out)
}

/// An `&optional` slot of a list pattern: `p`, or `(p default)` — the pattern's binders are
/// renamed, the default walked as a reference (it is evaluated before `p` binds).
fn hyg_optional_slot(heap: &mut Heap, slot: Value, scope: &mut HygScope, start: usize) -> Value {
    match heap.list_items(slot).as_deref() {
        Some(&[pat, default]) => {
            let default = hyg_walk(heap, default, scope);
            let pat = hyg_pattern(heap, pat, scope, start);
            rebuild_list(heap, slot, vec![pat, default])
        }
        _ => hyg_pattern(heap, slot, scope, start),
    }
}

/// One `bytes` segment: `x` binds a byte; `(x n)` binds `x` to `n` bytes, where a symbol
/// `n` is an earlier binding (a reference); `(x :u16)` a typed read; a literal is matched.
fn hyg_bytes_segment(heap: &mut Heap, segment: Value, scope: &mut HygScope, start: usize) -> Value {
    match segment.unpack() {
        ValueRef::Sym(s) => hyg_bind_sym(s, scope, start),
        ValueRef::Pair(_) => {
            let parts = match heap.list_items(segment) {
                Some(p) if !p.is_empty() => p,
                _ => return segment,
            };
            let mut out = Vec::with_capacity(parts.len());
            out.push(match parts[0].unpack() {
                ValueRef::Sym(s) => hyg_bind_sym(s, scope, start),
                _ => parts[0],
            });
            for &size in &parts[1..] {
                out.push(hyg_walk(heap, size, scope));
            }
            rebuild_list(heap, segment, out)
        }
        _ => segment,
    }
}

/// A map pattern: `{:k pattern}` entries rename their sub-patterns; `:keys [a b]` names
/// stay literal (they are the keys looked up) but shadow; `:or {a default}` defaults are
/// references, resolved after every name the map binds.
fn hyg_map_pattern(heap: &mut Heap, id: value::MapId, scope: &mut HygScope, start: usize) -> Value {
    let entries = heap.map_entries(id);
    let keyword_named = |k: Value, name: &str| matches!(k.unpack(), ValueRef::Keyword(s) if value::symbol_name_ref(s) == name);
    let mut out = Vec::with_capacity(entries.len());
    for &(k, v) in &entries {
        if keyword_named(k, "or") {
            continue;
        }
        if keyword_named(k, "keys") {
            let mut names = Vec::new();
            collect_all_syms(heap, v, &mut names);
            for s in names {
                scope.push((s, s));
            }
            out.push((k, v));
        } else {
            let renamed = hyg_pattern(heap, v, scope, start);
            out.push((k, renamed));
        }
    }
    for &(k, v) in &entries {
        if keyword_named(k, "or") {
            let defaults = match v.unpack() {
                ValueRef::Map(defaults_id) => {
                    let pairs = heap.map_entries(defaults_id);
                    let mut renamed = Vec::with_capacity(pairs.len());
                    for (name, default) in pairs {
                        let default = hyg_walk(heap, default, scope);
                        renamed.push((name, default));
                    }
                    heap.map_from_pairs(renamed)
                }
                _ => v,
            };
            out.push((k, defaults));
        }
    }
    heap.map_from_pairs(out)
}

// ---- the core binders ----------------------------------------------------------------

/// `(let/letrec (p0 v0 …) body…)` — rename each binder pattern. `letrec` binders are all
/// in scope for every RHS and the body; plain `let` is sequential (a binder scopes only
/// the *later* RHSs and the body — matching `resolve_let`).
fn hyg_let(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let letrec = matches!(items.first().map(|v| v.unpack()),
        Some(ValueRef::Sym(h)) if value::symbol_is(h, kw::LETREC));
    let binds_form = items.get(1).copied().unwrap_or(Value::nil());
    // A computed binding list (`(let ~bindings …)`) — can't see its binders; recurse
    // generically (the `~bindings` hole is left verbatim, the body under `scope`).
    if hyg_is_unquote(heap, binds_form) {
        return hyg_generic(heap, form, items, scope);
    }
    let binds = match heap.list_or_vector_items(binds_form) {
        Some(b) if b.len() % 2 == 0 => b,
        _ => return hyg_generic(heap, form, items, scope),
    };
    let mut new_scope = scope.to_vec();
    let mut new_binds = Vec::with_capacity(binds.len());
    if letrec {
        let mut targets = Vec::with_capacity(binds.len() / 2);
        for &t in binds.iter().step_by(2) {
            let start = new_scope.len();
            targets.push(hyg_pattern(heap, t, &mut new_scope, start));
        }
        for (target, &rhs) in targets.into_iter().zip(binds.iter().skip(1).step_by(2)) {
            new_binds.push(target);
            new_binds.push(hyg_walk(heap, rhs, &new_scope));
        }
    } else {
        for pair in binds.chunks(2) {
            let rhs = hyg_walk(heap, pair[1], &new_scope);
            let start = new_scope.len(); // sequential: bind AFTER the RHS
            new_binds.push(hyg_pattern(heap, pair[0], &mut new_scope, start));
            new_binds.push(rhs);
        }
    }
    let new_bind_form = rebuild_seq_like(heap, binds_form, new_binds);
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]);
    out.push(new_bind_form);
    for &b in items.get(2..).unwrap_or(&[]) {
        out.push(hyg_walk(heap, b, &new_scope));
    }
    rebuild_list(heap, form, out)
}

/// The parts of a `fn` after its head — single-arity `(params body…)` or multi-arity
/// `(doc? (params body…)…)`. Params bind together in their body. Mirrors `resolve_fn`.
fn hyg_fn_parts(
    heap: &mut Heap,
    parts: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
) -> Vec<Value> {
    let (has_doc, clause_start) = match parts.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if parts.len() > 1 => (true, 1),
        _ => (false, 0),
    };
    let clauses = &parts[clause_start..];
    let multi = !clauses.is_empty() && clauses.iter().all(|&f| is_arity_clause(heap, f));
    let mut out = Vec::with_capacity(parts.len());
    if multi {
        if has_doc {
            out.push(parts[0]);
        }
        for &clause in clauses {
            out.push(hyg_arity_clause(heap, clause, scope));
        }
    } else {
        let params = parts.first().copied().unwrap_or(Value::nil());
        let mut inner = scope.to_vec();
        out.push(hyg_param_list(heap, params, &mut inner));
        for &b in parts.get(1..).unwrap_or(&[]) {
            out.push(hyg_walk(heap, b, &inner));
        }
    }
    out
}

fn hyg_arity_clause(
    heap: &mut Heap,
    clause: Value,
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let cparts = match heap.list_items(clause) {
        Some(c) if !c.is_empty() => c,
        _ => return clause,
    };
    let mut inner = scope.to_vec();
    let new_params = hyg_param_list(heap, cparts[0], &mut inner);
    let mut out = Vec::with_capacity(cparts.len());
    out.push(new_params);
    for &b in &cparts[1..] {
        out.push(hyg_walk(heap, b, &inner));
    }
    rebuild_list(heap, clause, out)
}

/// Rename a parameter list's binders — each param is a pattern — extending `inner` with
/// them (all bound together for the body). A computed param list (`~params`) is left
/// verbatim.
fn hyg_param_list(heap: &mut Heap, params: Value, inner: &mut HygScope) -> Value {
    if hyg_is_unquote(heap, params) {
        return params; // `(fn ~params …)` — binders not visible
    }
    let elems = match heap.list_or_vector_items(params) {
        Some(e) => e,
        None => return params,
    };
    let start = inner.len();
    let mut out = Vec::with_capacity(elems.len());
    // After `&optional`, a `(name default)` slot's default is an expression over the params
    // before it, not a pattern: read as one, its every name was renamed as a binder.
    let mut optional = false;
    for p in elems {
        match p.unpack() {
            ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => {
                optional = true;
                out.push(p);
            }
            ValueRef::Sym(s) if value::symbol_is(s, kw::AMP) => {
                optional = false;
                out.push(p);
            }
            _ if optional => out.push(hyg_optional_slot(heap, p, inner, start)),
            _ => out.push(hyg_pattern(heap, p, inner, start)),
        }
    }
    rebuild_seq_like(heap, params, out)
}

// ---- the standard library's binding macros ---------------------------------------------

fn hyg_shaped(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
    shape: Shape,
) -> Value {
    match shape {
        Shape::Pairs => hyg_pairs(heap, form, items, scope, false),
        Shape::PairsThenElse => hyg_pairs(heap, form, items, scope, true),
        Shape::Clauses(leading) => hyg_clauses(heap, form, items, scope, leading),
        Shape::Try => {
            let mut out = vec![items[0]];
            for &part in &items[1..] {
                let is_catch =
                    head_sym(heap, part).is_some_and(|h| value::symbol_name_ref(h) == "catch");
                out.push(if is_catch {
                    hyg_clause(heap, part, scope, 1)
                } else {
                    hyg_walk(heap, part, scope)
                });
            }
            rebuild_list(heap, form, out)
        }
        Shape::AsArrow => {
            if items.len() < 3 {
                return hyg_generic(heap, form, items, scope);
            }
            let expr = hyg_walk(heap, items[1], scope);
            let mut inner = scope.to_vec();
            let start = inner.len();
            let name = hyg_pattern(heap, items[2], &mut inner, start);
            let mut out = vec![items[0], expr, name];
            for &step in &items[3..] {
                out.push(hyg_walk(heap, step, &inner));
            }
            rebuild_list(heap, form, out)
        }
        Shape::Defn => {
            if items.len() < 3 {
                return hyg_generic(heap, form, items, scope);
            }
            let mut out = vec![items[0], hyg_walk(heap, items[1], scope)];
            out.extend(hyg_fn_parts(heap, &items[2..], scope));
            rebuild_list(heap, form, out)
        }
        Shape::Defmethod => {
            if items.len() < 4 {
                return hyg_generic(heap, form, items, scope);
            }
            let mut out = vec![items[0], hyg_walk(heap, items[1], scope), items[2]];
            out.extend(hyg_fn_parts(heap, &items[3..], scope));
            rebuild_list(heap, form, out)
        }
        Shape::Impl => {
            if items.len() < 3 {
                return hyg_generic(heap, form, items, scope);
            }
            let mut out = vec![items[0], hyg_walk(heap, items[1], scope), items[2]];
            for &method in &items[3..] {
                // `(op params body…)` — `op` names the ability's operation.
                out.push(match heap.list_items(method) {
                    Some(parts) if parts.len() >= 2 && !hyg_is_unquote(heap, method) => {
                        let mut renamed = vec![parts[0]];
                        renamed.extend(hyg_fn_parts(heap, &parts[1..], scope));
                        rebuild_list(heap, method, renamed)
                    }
                    _ => hyg_walk(heap, method, scope),
                });
            }
            rebuild_list(heap, form, out)
        }
    }
}

/// What a standard binding macro in a template binds, read from the same shape table the
/// rename uses: each item is a pattern, or a whole parameter list.
pub(super) enum ShapedBinder {
    Pattern(Value),
    Params(Value),
}

/// The binders of `items` — a list form inside a template — when its head is a standard
/// binding macro, else `None`. The resolver reads them so it leaves these names local:
/// qualified, a binder would no longer be renamed here (a qualified name never is). Computed
/// binders (`~params`) are skipped, as the rename skips them.
pub(super) fn shaped_binders(heap: &Heap, items: &[Value]) -> Option<Vec<ShapedBinder>> {
    let head = match items.first()?.unpack() {
        ValueRef::Sym(h) => h,
        _ => return None,
    };
    let shape = binding_shape(head)?;
    let mut binders = Vec::new();
    match shape {
        Shape::Pairs | Shape::PairsThenElse => {
            let binds_form = items.get(1).copied().unwrap_or(Value::nil());
            if !hyg_is_unquote(heap, binds_form) {
                let binds = heap.list_or_vector_items(binds_form).unwrap_or_default();
                for pair in binds.chunks(2) {
                    if !matches!(pair[0].unpack(), ValueRef::Keyword(_)) {
                        binders.push(ShapedBinder::Pattern(pair[0]));
                    }
                }
            }
        }
        Shape::Clauses(leading) => {
            for &item in items.iter().skip(leading) {
                let is_after =
                    head_sym(heap, item).is_some_and(|h| value::symbol_name_ref(h) == "after");
                if is_after || hyg_is_unquote(heap, item) {
                    continue;
                }
                if let Some(&pattern) = heap.list_items(item).as_deref().and_then(|p| p.first()) {
                    binders.push(ShapedBinder::Pattern(pattern));
                }
            }
        }
        Shape::Try => {
            for &part in &items[1..] {
                if head_sym(heap, part).is_some_and(|h| value::symbol_name_ref(h) == "catch")
                    && let Some(&pattern) = heap.list_items(part).as_deref().and_then(|p| p.get(1))
                {
                    binders.push(ShapedBinder::Pattern(pattern));
                }
            }
        }
        Shape::AsArrow => {
            if let Some(&name) = items.get(2) {
                binders.push(ShapedBinder::Pattern(name));
            }
        }
        Shape::Defn => fn_part_params(heap, items.get(2..).unwrap_or(&[]), &mut binders),
        Shape::Defmethod => fn_part_params(heap, items.get(3..).unwrap_or(&[]), &mut binders),
        Shape::Impl => {
            for &method in items.get(3..).unwrap_or(&[]) {
                if hyg_is_unquote(heap, method) {
                    continue;
                }
                if let Some(parts) = heap.list_items(method).filter(|parts| parts.len() >= 2) {
                    fn_part_params(heap, &parts[1..], &mut binders);
                }
            }
        }
    }
    Some(binders)
}

/// The parameter lists of `fn`'s parts `doc? params body…` / `doc? (params body…)…`.
fn fn_part_params(heap: &Heap, parts: &[Value], binders: &mut Vec<ShapedBinder>) {
    let clause_start = match parts.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if parts.len() > 1 => 1,
        _ => 0,
    };
    let clauses = &parts[clause_start..];
    let params: Vec<Value> =
        if !clauses.is_empty() && clauses.iter().all(|&f| is_arity_clause(heap, f)) {
            clauses
                .iter()
                .filter_map(|&clause| heap.list_items(clause)?.first().copied())
                .collect()
        } else {
            clauses.first().copied().into_iter().collect()
        };
    for params in params {
        if !hyg_is_unquote(heap, params) {
            binders.push(ShapedBinder::Params(params));
        }
    }
}

/// `(head (pattern expr … :modifier expr …) body…)`. With `then_else`, only the first body
/// form (`then`) sees the bindings; the rest (`else`) are outside them.
fn hyg_pairs(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
    then_else: bool,
) -> Value {
    let binds_form = items.get(1).copied().unwrap_or(Value::nil());
    if hyg_is_unquote(heap, binds_form) {
        return hyg_generic(heap, form, items, scope);
    }
    let binds = match heap.list_or_vector_items(binds_form) {
        Some(b) => b,
        None => return hyg_generic(heap, form, items, scope),
    };
    let mut inner = scope.to_vec();
    let mut new_binds = Vec::with_capacity(binds.len());
    let mut i = 0;
    while i < binds.len() {
        let Some(&next) = binds.get(i + 1) else {
            // An odd tail — malformed; leave it for the macro to report.
            new_binds.push(binds[i]);
            break;
        };
        if matches!(binds[i].unpack(), ValueRef::Keyword(_)) {
            // `:when guard`, `:into coll` — an expression over the bindings so far.
            new_binds.push(binds[i]);
            new_binds.push(hyg_walk(heap, next, &inner));
        } else {
            let expr = hyg_walk(heap, next, &inner);
            let start = inner.len();
            new_binds.push(hyg_pattern(heap, binds[i], &mut inner, start));
            new_binds.push(expr);
        }
        i += 2;
    }
    let new_bind_form = rebuild_seq_like(heap, binds_form, new_binds);
    let mut out = vec![items[0], new_bind_form];
    for (index, &b) in items.iter().enumerate().skip(2) {
        let sees_bindings = !then_else || index == 2;
        out.push(hyg_walk(
            heap,
            b,
            if sees_bindings { &inner } else { scope },
        ));
    }
    rebuild_list(heap, form, out)
}

/// `(head x… clause…)` — the first `leading` items (head included) are expressions, the
/// rest `(pattern [:when guard] body…)` clauses.
fn hyg_clauses(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
    leading: usize,
) -> Value {
    let mut out = vec![items[0]];
    for (index, &item) in items.iter().enumerate().skip(1) {
        let is_after = head_sym(heap, item).is_some_and(|h| value::symbol_name_ref(h) == "after");
        out.push(
            if index < leading || is_after || hyg_is_unquote(heap, item) {
                hyg_walk(heap, item, scope)
            } else {
                hyg_clause(heap, item, scope, 0)
            },
        );
    }
    rebuild_list(heap, form, out)
}

/// One clause whose item at `at` is a pattern binding everything after it: a match clause
/// `(pattern [:when guard] body…)` (`at` 0), or `(catch e handler…)` (`at` 1).
fn hyg_clause(
    heap: &mut Heap,
    clause: Value,
    scope: &[(value::Symbol, value::Symbol)],
    at: usize,
) -> Value {
    let parts = match heap.list_items(clause) {
        Some(p) if p.len() > at => p,
        _ => return hyg_walk(heap, clause, scope),
    };
    let mut inner = scope.to_vec();
    let start = inner.len();
    let mut out = Vec::with_capacity(parts.len());
    out.extend(parts.iter().take(at).copied());
    out.push(hyg_pattern(heap, parts[at], &mut inner, start));
    for &b in &parts[at + 1..] {
        out.push(hyg_walk(heap, b, &inner));
    }
    rebuild_list(heap, clause, out)
}

/// True if `v` contains a form that binds — a `let`/`letrec`/`fn` or a standard binding
/// macro — so the static-quasiquote optimisation must defer to the runtime expand path
/// (fresh gensyms per expansion). Skips `quote`/`unquote` subtrees (a `let` in caller code or
/// quoted data isn't the template's). Conservative: a binding form whose binders are all `_`
/// or computed also takes the runtime path, and so does a nested template, whose holes back
/// to this level may bind (ADR-503).
pub(super) fn template_introduces_binder(heap: &Heap, v: Value) -> bool {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        template_introduces_binder_inner(heap, v)
    })
}

fn template_introduces_binder_inner(heap: &Heap, v: Value) -> bool {
    match v.unpack() {
        ValueRef::Pair(_) => {
            let items = match heap.list_items(v) {
                Some(i) => i,
                None => return false,
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|x| x.unpack()) {
                if value::symbol_is(h, kw::QUOTE)
                    || value::symbol_is(h, kw::UNQUOTE)
                    || value::symbol_is(h, kw::UNQUOTE_SPLICING)
                {
                    return false;
                }
                if value::symbol_is(h, kw::LET)
                    || value::symbol_is(h, kw::LETREC)
                    || value::symbol_is(h, kw::FN)
                    || value::symbol_is(h, kw::QUASIQUOTE)
                    || binding_shape(h).is_some()
                {
                    return true;
                }
            }
            items.iter().any(|&it| template_introduces_binder(heap, it))
        }
        ValueRef::Vector(id) => heap
            .vector(id)
            .to_vec()
            .iter()
            .any(|&it| template_introduces_binder(heap, it)),
        ValueRef::Map(id) => heap.map_entries(id).iter().any(|(k, val)| {
            template_introduces_binder(heap, *k) || template_introduces_binder(heap, *val)
        }),
        ValueRef::Set(id) => heap
            .set_elems(id)
            .iter()
            .any(|&e| template_introduces_binder(heap, e)),
        _ => false,
    }
}
