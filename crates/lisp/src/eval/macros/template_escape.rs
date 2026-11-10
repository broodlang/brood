//! Automatic root escapes in macro templates (ADR-423): a `defmacro` template's literal
//! call heads that name a root binding are rewritten to `/name` when the macro is defined.

use super::*;

// ============================================================================
// Template call heads pin to root (ADR-423)
// ============================================================================
//
// A macro's expansion is spliced into its CALLER's file, where a bare head resolves against
// the caller's namespace first. Once a module's own definition wins over a root name inside
// that module (ADR-423), a prelude template that emits a bare `when` or `first` would be
// captured by any module defining one. ADR-236 gave templates the `/name` root escape to
// write by hand; this pass writes it for them, the way Clojure's syntax-quote qualifies every
// template symbol to the defining namespace.
//
// Scope, chosen to be sound rather than complete:
// - Only quasiquote templates lexically inside a macro's function — what `defmacro` expands
//   to, `(%make-macro (fn …))`, is where the pass looks. A quasiquote elsewhere
//   builds DATA (a message, a form to print), whose symbols travel by name (ADR-034).
// - Only CALL HEADS — the first element of a template list. An argument naming a root
//   function (`(map xs first)`) is left as written; the hand-written `/first` covers it.
// - Never a binder. A head is left bare when the same symbol appears in any binder position
//   of the template (a `let`/`fn`/`doseq`/… binding list, a parameter list, a `match`/
//   `receive` pattern) — over-approximated over the whole template, so a binder the walk
//   misreads can only leave a head unescaped, never rename a local.
// - Never a special form: those are bound nowhere, so "names a root binding" excludes them.
// - `%name`s are not escaped (a user cannot define one, ADR-250), nor `defdyn`-declared
//   ambient names (always root), nor `x#` auto-gensyms or the `~`/`~@` markers.
// - `~` holes are macro-time code: not escaped, but a quasiquote nested inside one is
//   another template of the same macro and is escaped in turn.
// - A template nested in a template (ADR-503) is escaped at every level: its heads are what
//   the code the macro builds will emit. Only a hole out of the outermost level is macro-time
//   code.

/// Forms whose second element is a binding list of `binder expr` pairs.
const PAIR_BINDERS: &[&str] = &[
    "let",
    "letrec",
    "binding",
    "dolist",
    "doseq",
    "dotimes",
    "fold-for",
    "for",
    "if-let",
    "if-some",
    "when-let",
    "when-some",
    "with",
];

/// Forms whose clauses are `(pattern body…)` after one scrutinee.
const PATTERN_CLAUSES: &[&str] = &["match", "%match", "match*", "receive"];

/// The head's name, a root escape read as the name it escapes (`/%match` is `%match`).
fn head_name(items: &[Value]) -> Option<String> {
    match items.first().map(|v| v.unpack()) {
        Some(ValueRef::Sym(s)) => {
            let name = value::symbol_name(s);
            Some(match name.strip_prefix('/') {
                Some(escaped) if !escaped.is_empty() => escaped.to_string(),
                _ => name,
            })
        }
        _ => None,
    }
}

fn is_seq_form(v: Value) -> bool {
    matches!(v.unpack(), ValueRef::Pair(_) | ValueRef::Vector(_))
}

/// Every symbol in `v`, deeply — a pattern or parameter list's bound names, over-approximated.
fn binder_syms(heap: &Heap, v: Value, out: &mut HashSet<Symbol>) {
    let mut all = Vec::new();
    collect_all_syms(heap, v, &mut all);
    out.extend(all);
}

/// The binders a `fn`/`defn`/`defmacro` form introduces: its parameter list, or — multi-arity —
/// each `(params body…)` clause's parameter list.
fn fn_binders(heap: &Heap, items: &[Value], out: &mut HashSet<Symbol>) {
    for &it in &items[1..] {
        let Some(inner) = heap.list_or_vector_items(it) else {
            continue; // the name or the docstring
        };
        match inner.first() {
            // a clause `((params) body…)`: its first element is the parameter list
            Some(&params) if matches!(it.unpack(), ValueRef::Pair(_)) && is_seq_form(params) => {
                binder_syms(heap, params, out);
            }
            // the single parameter list itself, which ends the header
            _ => {
                binder_syms(heap, it, out);
                return;
            }
        }
    }
}

/// Collect the template's binder names (see the module comment). Holes and quoted data are
/// skipped: a hole's binders are macro-time locals, not template structure.
fn collect_template_binders(heap: &Heap, form: Value, level: u32, out: &mut HashSet<Symbol>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = heap.list_or_vector_items(form) else {
            return;
        };
        let mut level = level;
        if matches!(form.unpack(), ValueRef::Pair(_))
            && let Some(h) = head_name(&items)
        {
            match h.as_str() {
                "quote" => return,
                "unquote" | "unquote-splicing" if level == 1 => return,
                // a nested template (ADR-503): its binders, and those of its holes back to
                // this level, are the template's too
                "unquote" | "unquote-splicing" => level -= 1,
                "quasiquote" => level += 1,
                "fn" | "defn" | "defn-" | "defmacro" => fn_binders(heap, &items, out),
                h if PAIR_BINDERS.contains(&h) => {
                    if let Some(binds) = items.get(1).and_then(|b| heap.list_or_vector_items(*b)) {
                        for b in binds.iter().step_by(2) {
                            binder_syms(heap, *b, out);
                        }
                    }
                }
                h if PATTERN_CLAUSES.contains(&h) => {
                    for clause in items.iter().skip(2) {
                        if let Some(c) = heap.list_or_vector_items(*clause)
                            && let Some(p) = c.first()
                        {
                            binder_syms(heap, *p, out);
                        }
                    }
                }
                _ => {}
            }
        }
        for it in items {
            collect_template_binders(heap, it, level, out);
        }
    })
}

/// Should the template head `s` be written `/s`?
fn escapes(heap: &Heap, s: Symbol, binders: &HashSet<Symbol>) -> bool {
    let name = value::symbol_name_ref(s);
    !(name.is_empty()
        || name.contains('/')
        || name.starts_with('%')
        || name.starts_with('&')
        || (name.len() > 1 && name.ends_with('#'))
        || value::symbol_is(s, kw::UNQUOTE)
        || value::symbol_is(s, kw::UNQUOTE_SPLICING)
        || binders.contains(&s)
        || is_ambient(heap, s))
        && heap.env_get(heap.global(), s).is_some()
}

/// Rewrite one template (the argument of a `quasiquote`) at quasiquote `level`: 1 in the
/// template itself, more inside a nested one, whose heads are escaped too (ADR-503).
fn escape_template(heap: &mut Heap, form: Value, level: u32, binders: &HashSet<Symbol>) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form.unpack() {
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(form) else {
                return form; // improper (dotted) list: leave verbatim
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
                if value::symbol_is(h, kw::QUOTE) {
                    return form;
                }
                let is_hole =
                    value::symbol_is(h, kw::UNQUOTE) || value::symbol_is(h, kw::UNQUOTE_SPLICING);
                if items.len() == 2 && (value::symbol_is(h, kw::QUASIQUOTE) || is_hole && level > 1)
                {
                    let inner_level = if is_hole { level - 1 } else { level + 1 };
                    let held = escape_template(heap, items[1], inner_level, binders);
                    return rebuild_list(heap, form, vec![items[0], held]);
                }
                if is_hole {
                    // macro-time code: only its own nested templates are escaped
                    let mut out = Vec::with_capacity(items.len());
                    out.push(items[0]);
                    for &it in &items[1..] {
                        out.push(escape_macro_code(heap, it));
                    }
                    return rebuild_list(heap, form, out);
                }
            }
            let mut out = Vec::with_capacity(items.len());
            for (i, &it) in items.iter().enumerate() {
                let it = match it.unpack() {
                    ValueRef::Sym(s) if i == 0 && escapes(heap, s, binders) => {
                        Value::symbol(value::intern(&format!("/{}", value::symbol_name_ref(s))))
                    }
                    _ => escape_template(heap, it, level, binders),
                };
                out.push(it);
            }
            rebuild_list(heap, form, out)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out: Vec<Value> = items
                .into_iter()
                .map(|v| escape_template(heap, v, level, binders))
                .collect();
            heap.alloc_vector(out)
        }
        _ => form,
    })
}

/// Walk macro-time code (a `defmacro` body, or a `~` hole), escaping every quasiquote
/// template it builds.
pub(super) fn escape_macro_code(heap: &mut Heap, form: Value) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match form.unpack() {
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(form) else {
                return form;
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
                if value::symbol_is(h, kw::QUOTE) {
                    return form;
                }
                if value::symbol_is(h, kw::QUASIQUOTE) {
                    let mut binders = HashSet::new();
                    for &t in &items[1..] {
                        collect_template_binders(heap, t, 1, &mut binders);
                    }
                    let mut out = Vec::with_capacity(items.len());
                    out.push(items[0]);
                    for &t in &items[1..] {
                        out.push(escape_template(heap, t, 1, &binders));
                    }
                    return rebuild_list(heap, form, out);
                }
            }
            let out: Vec<Value> = items
                .into_iter()
                .map(|v| escape_macro_code(heap, v))
                .collect();
            rebuild_list(heap, form, out)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out: Vec<Value> = items
                .into_iter()
                .map(|v| escape_macro_code(heap, v))
                .collect();
            heap.alloc_vector(out)
        }
        _ => form,
    })
}

/// Find every macro definition in a resolved top-level form — `defmacro` has expanded to
/// `(def name (%make-macro (fn …)))` by now — and escape the templates its function builds.
/// A `%make-macro` inside quoted data or a template is data, not a definition.
pub(super) fn escape_macro_definitions(heap: &mut Heap, form: Value) -> Value {
    if !mentions_make_macro(heap, form) {
        return form; // the common case: hand back the same tree, allocating nothing
    }
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    escape_definitions_in(heap, form)
}

fn is_make_macro(s: Symbol) -> bool {
    value::symbol_name_ref(s) == "%make-macro"
}

fn mentions_make_macro(heap: &Heap, form: Value) -> bool {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let ValueRef::Pair(_) = form.unpack() else {
            return false;
        };
        let Some(items) = heap.list_or_vector_items(form) else {
            return false;
        };
        match items.first().map(|v| v.unpack()) {
            Some(ValueRef::Sym(h)) if is_make_macro(h) => true,
            Some(ValueRef::Sym(h))
                if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) =>
            {
                false
            }
            _ => items.iter().any(|v| mentions_make_macro(heap, *v)),
        }
    })
}

fn escape_definitions_in(heap: &mut Heap, form: Value) -> Value {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let ValueRef::Pair(_) = form.unpack() else {
            return form;
        };
        let Some(items) = heap.list_items(form) else {
            return form;
        };
        match items.first().map(|v| v.unpack()) {
            Some(ValueRef::Sym(h)) if is_make_macro(h) => {
                let mut out = Vec::with_capacity(items.len());
                out.push(items[0]);
                for &it in &items[1..] {
                    out.push(escape_macro_code(heap, it));
                }
                rebuild_list(heap, form, out)
            }
            Some(ValueRef::Sym(h))
                if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) =>
            {
                form
            }
            _ => {
                let out: Vec<Value> = items
                    .into_iter()
                    .map(|v| escape_definitions_in(heap, v))
                    .collect();
                rebuild_list(heap, form, out)
            }
        }
    })
}
