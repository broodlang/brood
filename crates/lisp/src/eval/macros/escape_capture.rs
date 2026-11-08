//! A `/name` root escape reaches the root binding even under a local of the same name: a
//! local binder that would capture one is renamed.

use super::resolve::{collect_all_syms, is_root_escape, rebuild_seq_like};
use super::*;

// ============================================================================
// Root escapes are immune to local capture
// ============================================================================
//
// `/name` means "the root binding of `name`" (ADR-236), and macro templates lean on it: a
// prelude macro's call heads are escaped for it (ADR-423), and the pattern compiler writes
// `(/rest tl)` by hand. The resolver strips the escape to a bare `name` before the code is
// compiled, so a *local* `name` in scope would still capture it. That is the classic hygiene
// failure — a binder the CALLER wrote, placed by the macro around the macro's own code:
//
//     (match xs ((x & rest) y) …)   ; the generated walk calls `rest`, now the caller's list
//
// This pass runs on the fully expanded form, before the escapes are stripped. A local binder
// whose name some code-level `/name` in the form escapes is alpha-renamed, with every
// reference it binds; the escape is then stripped to a bare name no local captures.
//
// - Only the core binders survive expansion — `let`, `letrec`, `fn` (single- or arity
//   multi-clause; patterns are lowered by now), `defmacro` — so the walk models every
//   binding site there is. A target it cannot read is shadowed as written, never renamed.
// - Renaming a binder together with the references it binds preserves meaning, so the pass
//   is sound whatever it renames; it renames only names an escape in the form spells.
// - Templates are data: their symbols are left alone, but a `~` hole back to the code level
//   is code and takes the renames in scope.
// - The fresh name is a gensym, so a parameter list, an arity error or a debugger shows the
//   name as written (`value::source_symbol`).

/// Rename every local binder of `form` that would capture one of its root escapes. Returns
/// `form` itself, unallocated, when the form escapes nothing — the common case.
pub(super) fn rename_escape_captures(heap: &mut Heap, form: Value) -> Value {
    let mut escaped = Vec::new();
    collect_escapes(heap, form, 0, &mut escaped);
    // Escapes are common — every pattern's walk, every fused fold — and a binder spelled like
    // one is rare, so ask that read-only before rebuilding anything.
    if escaped.is_empty() || !binds_any(heap, form, &escaped, 0) {
        return form;
    }
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    Renamer { escaped }.code(heap, form, &mut Vec::new())
}

/// The names the code-level `/name` escapes in `v` spell, at quasiquote `level` (0 = code).
fn collect_escapes(heap: &Heap, v: Value, level: u32, out: &mut Vec<value::Symbol>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match v.unpack() {
        ValueRef::Sym(s) if level == 0 && is_root_escape(s) => {
            if let Some(name) = value::intern_existing(&value::symbol_name_ref(s)[1..])
                && !out.contains(&name)
            {
                out.push(name);
            }
        }
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(v) else {
                return;
            };
            match nesting(&items, level) {
                Nesting::Data => {}
                Nesting::Enter(inner) => collect_escapes(heap, items[1], inner, out),
                Nesting::Same => {
                    for item in items {
                        collect_escapes(heap, item, level, out);
                    }
                }
            }
        }
        ValueRef::Vector(_) | ValueRef::Map(_) | ValueRef::Set(_) => {
            for item in elements(heap, v) {
                collect_escapes(heap, item, level, out);
            }
        }
        _ => {}
    })
}

/// Does any code-level binder in `v` (a `let`/`letrec` target, a `fn`/`defmacro` parameter,
/// typed or not) spell one of `names`? Over-approximates by treating every symbol of a binding
/// list or parameter list as a binder — a `true` only costs the rename walk.
fn binds_any(heap: &Heap, v: Value, names: &[value::Symbol], level: u32) -> bool {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match v.unpack() {
        ValueRef::Pair(_) => {
            let Some(items) = heap.list_items(v) else {
                return false;
            };
            match nesting(&items, level) {
                Nesting::Data => return false,
                Nesting::Enter(inner) => return binds_any(heap, items[1], names, inner),
                Nesting::Same => {}
            }
            if level == 0
                && let Some(ValueRef::Sym(head)) = items.first().map(|h| h.unpack())
            {
                let binder_lists: &[Value] = if value::symbol_is(head, kw::LET)
                    || value::symbol_is(head, kw::LETREC)
                    || value::symbol_is(head, kw::FN)
                {
                    items.get(1..2).unwrap_or(&[])
                } else if value::symbol_is(head, kw::DEFMACRO) {
                    items.get(2..3).unwrap_or(&[])
                } else {
                    &[]
                };
                let mut binders = Vec::new();
                for &list in binder_lists {
                    collect_all_syms(heap, list, &mut binders);
                }
                // A multi-clause fn's clauses are `(params body…)`: their params are in its
                // items, which the recursion below reaches as ordinary lists — so read each
                // clause's head list here too.
                if value::symbol_is(head, kw::FN) && fn_is_arity_multi_clause(heap, &items) {
                    for &clause in &items[1..] {
                        if let Some(first) =
                            heap.list_items(clause).and_then(|c| c.first().copied())
                        {
                            collect_all_syms(heap, first, &mut binders);
                        }
                    }
                }
                if binders.iter().any(|&b| names.contains(&binder_base(b))) {
                    return true;
                }
            }
            items
                .iter()
                .any(|&item| binds_any(heap, item, names, level))
        }
        ValueRef::Vector(_) | ValueRef::Map(_) | ValueRef::Set(_) => elements(heap, v)
            .into_iter()
            .any(|item| binds_any(heap, item, names, level)),
        _ => false,
    })
}

/// A binder's name without a type annotation: `x:int` → `x`.
fn binder_base(s: value::Symbol) -> value::Symbol {
    let spelling = value::symbol_name_ref(s);
    match spelling.find(':') {
        Some(index) if index > 0 => value::intern(&spelling[..index]),
        _ => s,
    }
}

/// How a list at quasiquote `level` changes the level of what it holds.
pub(super) enum Nesting {
    /// A `quote` at the code level: data, nothing in it is code.
    Data,
    /// `` `x `` or `~x`: its one argument is at this level.
    Enter(u32),
    /// Anything else: its elements are at the list's own level.
    Same,
}

pub(super) fn nesting(items: &[Value], level: u32) -> Nesting {
    let Some(ValueRef::Sym(head)) = items.first().map(|v| v.unpack()) else {
        return Nesting::Same;
    };
    if level == 0 && value::symbol_is(head, kw::QUOTE) {
        Nesting::Data
    } else if items.len() == 2 && value::symbol_is(head, kw::QUASIQUOTE) {
        Nesting::Enter(level + 1)
    } else if level > 0
        && items.len() == 2
        && (value::symbol_is(head, kw::UNQUOTE) || value::symbol_is(head, kw::UNQUOTE_SPLICING))
    {
        Nesting::Enter(level - 1)
    } else {
        Nesting::Same
    }
}

/// The elements of a vector, a map (keys and values) or a set.
pub(super) fn elements(heap: &Heap, v: Value) -> Vec<Value> {
    match v.unpack() {
        ValueRef::Vector(id) => heap.vector(id).to_vec(),
        ValueRef::Map(id) => heap
            .map_entries(id)
            .into_iter()
            .flat_map(|(key, val)| [key, val])
            .collect(),
        ValueRef::Set(id) => heap.set_elems(id),
        _ => Vec::new(),
    }
}

/// A scope: binder name → the symbol its references become (itself when not renamed, which
/// still shadows an outer rename). Innermost — last — wins.
type Scope = Vec<(value::Symbol, value::Symbol)>;

struct Renamer {
    escaped: Vec<value::Symbol>,
}

impl Renamer {
    /// Rewrite code `v` under `scope`.
    fn code(&self, heap: &mut Heap, v: Value, scope: &mut Scope) -> Value {
        crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match v.unpack() {
            ValueRef::Sym(s) => match scope.iter().rev().find(|(name, _)| *name == s) {
                Some((_, target)) => Value::symbol(*target),
                None => v,
            },
            ValueRef::Pair(_) => self.list(heap, v, scope),
            ValueRef::Vector(_) | ValueRef::Map(_) | ValueRef::Set(_) => {
                self.collection(heap, v, |this, heap, item| this.code(heap, item, scope))
            }
            _ => v,
        })
    }

    /// Rewrite template `v` at quasiquote `level` (1 just inside a `` ` ``): only its holes
    /// back to the code level are code.
    fn template(&self, heap: &mut Heap, v: Value, level: u32, scope: &mut Scope) -> Value {
        crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match v.unpack() {
            ValueRef::Pair(_) => {
                let Some(items) = heap.list_items(v) else {
                    return v;
                };
                let out = match nesting(&items, level) {
                    Nesting::Enter(0) => vec![items[0], self.code(heap, items[1], scope)],
                    Nesting::Enter(inner) => {
                        vec![items[0], self.template(heap, items[1], inner, scope)]
                    }
                    Nesting::Data | Nesting::Same => items
                        .iter()
                        .map(|&item| self.template(heap, item, level, scope))
                        .collect(),
                };
                rebuild_list(heap, v, out)
            }
            ValueRef::Vector(_) | ValueRef::Map(_) | ValueRef::Set(_) => {
                self.collection(heap, v, |this, heap, item| {
                    this.template(heap, item, level, scope)
                })
            }
            _ => v,
        })
    }

    fn collection(
        &self,
        heap: &mut Heap,
        v: Value,
        mut rewrite: impl FnMut(&Self, &mut Heap, Value) -> Value,
    ) -> Value {
        match v.unpack() {
            ValueRef::Vector(id) => {
                let items = heap.vector(id).to_vec();
                let out = items
                    .into_iter()
                    .map(|item| rewrite(self, heap, item))
                    .collect();
                heap.alloc_vector(out)
            }
            ValueRef::Map(id) => {
                let entries = heap.map_entries(id);
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, val) in entries {
                    let key = rewrite(self, heap, key);
                    let val = rewrite(self, heap, val);
                    pairs.push((key, val));
                }
                heap.map_from_pairs(pairs)
            }
            ValueRef::Set(id) => {
                let items = heap.set_elems(id);
                let out = items
                    .into_iter()
                    .map(|item| rewrite(self, heap, item))
                    .collect();
                heap.set_from_elems(out)
            }
            _ => v,
        }
    }

    fn list(&self, heap: &mut Heap, form: Value, scope: &mut Scope) -> Value {
        let Some(items) = heap.list_items(form) else {
            return form;
        };
        let head = match items.first().map(|v| v.unpack()) {
            Some(ValueRef::Sym(head)) => Some(head),
            _ => None,
        };
        let is = |name: &str| head.is_some_and(|head| value::symbol_is(head, name));
        if is(kw::QUOTE) {
            return form;
        }
        let out = if is(kw::QUASIQUOTE) && items.len() == 2 {
            vec![items[0], self.template(heap, items[1], 1, scope)]
        } else if is(kw::LET) && items.len() >= 2 {
            self.let_form(heap, &items, scope, false)
        } else if is(kw::LETREC) && items.len() >= 2 {
            self.let_form(heap, &items, scope, true)
        } else if is(kw::FN) && items.len() >= 2 {
            self.fn_form(heap, &items, scope)
        } else if is(kw::DEFMACRO) && items.len() >= 3 {
            // `(defmacro name params body…)`: the name is a global, the params a clause.
            let mut out = vec![items[0], items[1]];
            out.extend(self.clause(heap, &items[2..], scope));
            out
        } else {
            items
                .iter()
                .map(|&item| self.code(heap, item, scope))
                .collect()
        };
        rebuild_list(heap, form, out)
    }

    /// `(let (target expr …) body…)` — sequential, each target in scope for the later
    /// exprs and the body; `letrec` puts every target in scope for every expr.
    fn let_form(
        &self,
        heap: &mut Heap,
        items: &[Value],
        scope: &mut Scope,
        recursive: bool,
    ) -> Vec<Value> {
        let Some(bindings) = heap.list_or_vector_items(items[1]) else {
            return items
                .iter()
                .map(|&item| self.code(heap, item, scope))
                .collect();
        };
        let mark = scope.len();
        let mut out_bindings = bindings.clone();
        if recursive {
            for index in (0..bindings.len()).step_by(2) {
                out_bindings[index] = self.bind(heap, bindings[index], scope);
            }
        }
        for index in (0..bindings.len()).step_by(2) {
            if let Some(&expr) = bindings.get(index + 1) {
                out_bindings[index + 1] = self.code(heap, expr, scope);
            }
            if !recursive {
                out_bindings[index] = self.bind(heap, bindings[index], scope);
            }
        }
        let mut out = vec![items[0], rebuild_seq_like(heap, items[1], out_bindings)];
        for &body in &items[2..] {
            out.push(self.code(heap, body, scope));
        }
        scope.truncate(mark);
        out
    }

    /// `(fn params body…)`, or an arity multi-clause `(fn doc? (params body…)…)`.
    fn fn_form(&self, heap: &mut Heap, items: &[Value], scope: &mut Scope) -> Vec<Value> {
        let mut out = vec![items[0]];
        if fn_is_arity_multi_clause(heap, items) {
            for &part in &items[1..] {
                match heap.list_items(part) {
                    Some(clause) if !matches!(part.unpack(), ValueRef::Str(_)) => {
                        let rewritten = self.clause(heap, &clause, scope);
                        out.push(rebuild_list(heap, part, rewritten));
                    }
                    _ => out.push(part), // the docstring
                }
            }
        } else {
            out.extend(self.clause(heap, &items[1..], scope));
        }
        out
    }

    /// `params body…` — the parameters in scope for the body (and for a later parameter's
    /// `&optional` default).
    fn clause(&self, heap: &mut Heap, parts: &[Value], scope: &mut Scope) -> Vec<Value> {
        let mark = scope.len();
        let params = match heap.list_items(parts[0]) {
            Some(params) => {
                let mut out_params = Vec::with_capacity(params.len());
                for param in params {
                    let rewritten = match param.unpack() {
                        ValueRef::Sym(s) if value::symbol_name_ref(s).starts_with('&') => param,
                        ValueRef::Sym(_) => self.bind(heap, param, scope),
                        // `&optional (name default)`: the default sees the earlier params.
                        ValueRef::Pair(_) => match heap.list_items(param).as_deref() {
                            Some(&[name, default]) => {
                                let default = self.code(heap, default, scope);
                                let name = self.bind(heap, name, scope);
                                heap.list(vec![name, default])
                            }
                            _ => self.bind(heap, param, scope),
                        },
                        _ => param,
                    };
                    out_params.push(rewritten);
                }
                rebuild_seq_like(heap, parts[0], out_params)
            }
            None => self.bind(heap, parts[0], scope), // `(fn args body…)` — one rest name
        };
        let mut out = vec![params];
        for &body in &parts[1..] {
            out.push(self.code(heap, body, scope));
        }
        scope.truncate(mark);
        out
    }

    /// Put binder `target` in scope and return what to emit in its place: a fresh name when
    /// an escape in the form spells it (a typed binder `x:int` keeps its `:int`), itself
    /// otherwise. A target that is not a symbol is shadowed as written.
    fn bind(&self, heap: &mut Heap, target: Value, scope: &mut Scope) -> Value {
        let ValueRef::Sym(s) = target.unpack() else {
            let mut names = Vec::new();
            collect_all_syms(heap, target, &mut names);
            scope.extend(names.into_iter().map(|name| (name, name)));
            return target;
        };
        let spelling = value::symbol_name_ref(s);
        let (base, annotation) = match spelling.find(':') {
            Some(index) if index > 0 => (value::intern(&spelling[..index]), &spelling[index..]),
            _ => (s, ""),
        };
        if !self.escaped.contains(&base) {
            scope.push((base, base));
            return target;
        }
        let fresh = match value::gensym(&value::symbol_name(base)).unpack() {
            ValueRef::Sym(fresh) => fresh,
            _ => base,
        };
        scope.push((base, fresh));
        if annotation.is_empty() {
            Value::symbol(fresh)
        } else {
            Value::symbol(value::intern(&format!(
                "{}{annotation}",
                value::symbol_name_ref(fresh)
            )))
        }
    }
}
