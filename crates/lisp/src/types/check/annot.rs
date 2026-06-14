//! `(sig name (params… -> ret))` type annotations — the parser from a Brood
//! type-expression *form* to a [`Ty`]/[`Sig`], plus the recogniser that pulls a
//! declaration out of a top-level form. See `docs/type-annotations.md`.
//!
//! Slice 1 is **checker-facing only**: a declared `Sig` is read by the checker as
//! an authoritative signature source (ahead of primitive / curated / inferred).
//! The `sig` form is a runtime no-op (a prelude macro expanding to `nil`), so the
//! scan runs over the *un-expanded* forms — like the hygiene lint. Nothing here
//! enforces the declaration at run time yet; that is slice 2 (the strong arrow).

use std::collections::HashMap;

use crate::core::heap::Heap;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

use super::ctx::{SigTerm, SigWithVars};
use super::walk::list_items;

/// The lattice point a base type *name* denotes — the spellings `type-of`
/// returns, plus the named unions (`number` = int∪float, `list` = nil∪pair,
/// `fn` = fn∪native). `None` for an unknown name, so an unrecognised annotation
/// is dropped rather than guessed (never a false signal).
fn base_ty(name: &str) -> Option<Ty> {
    Some(match name {
        "any" => Ty::ANY,
        "never" => Ty::NEVER,
        "int" => Ty::of(Tag::Int),
        "float" => Ty::of(Tag::Float),
        "number" => Ty::NUMBER,
        "string" => Ty::of(Tag::Str),
        "symbol" => Ty::of(Tag::Sym),
        "keyword" => Ty::of(Tag::Keyword),
        "bool" => Ty::of(Tag::Bool),
        "nil" => Ty::of(Tag::Nil),
        "pair" => Ty::of(Tag::Pair),
        "vector" => Ty::of(Tag::Vector),
        "list" => Ty::LIST,
        "map" => Ty::of(Tag::Map),
        "fn" => Ty::of(Tag::Fn).union(Ty::of(Tag::Native)),
        "rope" => Ty::of(Tag::Rope),
        "pid" => Ty::of(Tag::Pid),
        "ref" => Ty::of(Tag::Ref),
        "socket" => Ty::of(Tag::Socket),
        "subprocess" => Ty::of(Tag::Subprocess),
        "table" => Ty::of(Tag::Table),
        _ => return None,
    })
}

/// Parse a type-expression form to a [`Ty`]. Handles base names, type
/// variables (`?A` → `Ty::ANY`), arrows `(p… -> r)`, `(list E)` /
/// `(vector E)`, `(or A B …)`, `(and A B …)`, and `(map K V)` (flat
/// `Ty::Map` in slice 1). `None` for anything unrecognised — the annotation
/// is then dropped, never guessed.
pub(super) fn parse_type(heap: &Heap, form: Value) -> Option<Ty> {
    match form {
        Value::Sym(s) => {
            let name = value::symbol_name(s);
            // Type variables (`?A`, `?el`, etc.) — static-only, no runtime meaning.
            // Unknown to `type-matches?` → accepts everything (correct: it's a
            // static constraint, not a runtime one). Resolve to ANY here so the
            // checker uses the widest safe type at positions it can't unify.
            if name.starts_with('?') {
                return Some(Ty::ANY);
            }
            base_ty(&name)
        }
        // `nil` reads as the literal `Value::Nil`, not a symbol — so a type-expr
        // like `(or int nil)` lands here, not in `base_ty`.
        Value::Nil => Some(Ty::of(Tag::Nil)),
        // A bare keyword in type position is a literal (singleton) type — exactly
        // that value. Unambiguous (base types are bare *symbols*), and the form
        // `(or :maximized :fullboth nil)` composes via the `(or …)` union above.
        Value::Keyword(s) => Some(Ty::keyword_lit(s)),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            // An arrow: a list containing the `->` marker. Detect it first, so
            // `(int -> int)` isn't mistaken for an `(int …)` application.
            if let Some(pos) = items.iter().position(|v| is_arrow_marker(*v)) {
                return parse_arrow(heap, &items, pos).map(Ty::arrow);
            }
            let Value::Sym(head) = *items.first()? else {
                return None;
            };
            // (list E) / (vector E) — element-typed sequences.
            if value::symbol_is(head, "list") && items.len() == 2 {
                return Some(Ty::list_of(parse_type(heap, items[1])?));
            }
            if value::symbol_is(head, "vector") && items.len() == 2 {
                return Some(Ty::vector_of(parse_type(heap, items[1])?));
            }
            // (or A B …) — a union.
            if value::symbol_is(head, "or") && items.len() >= 2 {
                let mut acc: Option<Ty> = None;
                for &it in &items[1..] {
                    let t = parse_type(heap, it)?;
                    acc = Some(match acc {
                        Some(a) => a.union(t),
                        None => t,
                    });
                }
                return acc;
            }
            // (and A B …) — an intersection.  Ty::intersect is already
            // well-tested set intersection; no new Ty variant needed.
            // A bare (and) with no args is Ty::ANY (vacuously true).
            if value::symbol_is(head, "and") {
                if items.len() == 1 {
                    return Some(Ty::ANY);
                }
                let mut acc: Option<Ty> = None;
                for &it in &items[1..] {
                    let t = parse_type(heap, it)?;
                    acc = Some(match acc {
                        Some(a) => a.intersect(t),
                        None => t,
                    });
                }
                return acc;
            }
            // (map K V) — key/value typed map.  Full refinement: produce Ty::map_of
            // so the checker can derive `get`/`keys`/`vals`/`assoc` result types.
            if value::symbol_is(head, "map") && items.len() == 3 {
                let k = parse_type(heap, items[1])?;
                let v = parse_type(heap, items[2])?;
                return Some(Ty::map_of(k, v));
            }
            None
        }
        _ => None,
    }
}

fn is_arrow_marker(v: Value) -> bool {
    matches!(v, Value::Sym(s) if value::symbol_is(s, "->"))
}

/// Parse the items of an arrow type-expr (the `->` at index `pos`) to a [`Sig`]:
/// the items before `->` are parameter types, the single item after is the
/// result. A `&` marker splits fixed params from a variadic rest type, e.g.
/// `(int & number -> int)` → `Sig::with_rest([int], number, int)`. `None` if
/// malformed (no single result, or any part unparseable).
fn parse_arrow(heap: &Heap, items: &[Value], pos: usize) -> Option<Sig> {
    if pos + 2 != items.len() {
        return None; // exactly one result type must follow `->`
    }
    let ret = parse_type(heap, items[pos + 1])?;

    // Detect an optional `&` rest marker in the params, e.g. `(int & number -> r)`.
    let amp = items[..pos]
        .iter()
        .position(|v| matches!(v, Value::Sym(s) if value::symbol_is(*s, "&")));

    if let Some(apos) = amp {
        // Must be exactly one type after `&` before `->`.
        if apos + 2 != pos {
            return None;
        }
        let mut params = Vec::with_capacity(apos);
        for &p in &items[..apos] {
            params.push(parse_type(heap, p)?);
        }
        let rest = parse_type(heap, items[apos + 1])?;
        Some(Sig::with_rest(params, rest, ret))
    } else {
        let mut params = Vec::with_capacity(pos);
        for &p in &items[..pos] {
            params.push(parse_type(heap, p)?);
        }
        Some(Sig::new(params, ret))
    }
}

/// Parse a type-expression form into a [`SigTerm`], tracking type-variable
/// assignments in `vars` (variable name → sequential index). Every `?`-prefixed
/// symbol that hasn't been seen before gets the next index.
fn parse_type_term(heap: &Heap, form: Value, vars: &mut HashMap<String, u32>) -> Option<SigTerm> {
    match form {
        Value::Sym(s) => {
            let name = value::symbol_name(s);
            if name.starts_with('?') {
                let next = vars.len() as u32;
                let idx = *vars.entry(name.to_owned()).or_insert(next);
                return Some(SigTerm::Var(idx));
            }
            base_ty(&name).map(SigTerm::Ty)
        }
        Value::Nil => Some(SigTerm::Ty(Ty::of(Tag::Nil))),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            // Arrow markers are only valid at top level — skip nested arrows.
            if items.iter().any(|v| is_arrow_marker(*v)) {
                return None;
            }
            let Value::Sym(head) = *items.first()? else {
                return None;
            };
            if value::symbol_is(head, "list") && items.len() == 2 {
                let inner = parse_type_term(heap, items[1], vars)?;
                return Some(SigTerm::ListOf(Box::new(inner)));
            }
            if value::symbol_is(head, "vector") && items.len() == 2 {
                let inner = parse_type_term(heap, items[1], vars)?;
                return Some(SigTerm::VectorOf(Box::new(inner)));
            }
            // Compound forms without inner-var support — delegate to parse_type
            // (type vars inside `or`/`and`/`map` widen to Ty::ANY there).
            parse_type(heap, form).map(SigTerm::Ty)
        }
        _ => parse_type(heap, form).map(SigTerm::Ty),
    }
}

/// Parse the items of an arrow type-expr to a [`SigWithVars`], tracking type
/// variables in `vars`. Mirrors [`parse_arrow`] but produces `SigTerm`s.
fn parse_arrow_with_vars(
    heap: &Heap,
    items: &[Value],
    pos: usize,
    vars: &mut HashMap<String, u32>,
) -> Option<SigWithVars> {
    if pos + 2 != items.len() {
        return None;
    }
    let ret = parse_type_term(heap, items[pos + 1], vars)?;
    let amp = items[..pos]
        .iter()
        .position(|v| matches!(v, Value::Sym(s) if value::symbol_is(*s, "&")));
    let (params, rest) = if let Some(apos) = amp {
        if apos + 2 != pos {
            return None;
        }
        let mut params = Vec::with_capacity(apos);
        for &p in &items[..apos] {
            params.push(parse_type_term(heap, p, vars)?);
        }
        let rest_term = parse_type_term(heap, items[apos + 1], vars)?;
        (params, Some(rest_term))
    } else {
        let mut params = Vec::with_capacity(pos);
        for &p in &items[..pos] {
            params.push(parse_type_term(heap, p, vars)?);
        }
        (params, None)
    };
    Some(SigWithVars { params, rest, ret })
}

/// If `form` is a `(sig name (… -> …))` declaration whose arrow contains at
/// least one type variable (`?A`, `?B` …), return `(name, sig_with_vars)`.
/// Returns `None` for non-`sig` forms, non-arrow type-exprs, or arrows with
/// no variables — the plain [`parse_sig_decl`] path handles those.
pub(super) fn parse_sig_decl_with_vars(heap: &Heap, form: Value) -> Option<(Symbol, SigWithVars)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    let ty_items = list_items(heap, items[2])?;
    let pos = ty_items.iter().position(|v| is_arrow_marker(*v))?;
    let mut vars: HashMap<String, u32> = HashMap::new();
    let sig = parse_arrow_with_vars(heap, &ty_items, pos, &mut vars)?;
    if vars.is_empty() {
        return None;
    }
    Some((name, sig))
}

/// If `form` is a `(sig name (… -> …))` declaration whose type-expr is an arrow,
/// return `(name, sig)`. `None` for a non-`sig` form, a malformed one, or a
/// non-arrow type-expr (`(sig x int)` — accepted by the grammar but not a call
/// signature, so nothing to record in slice 1).
pub(super) fn parse_sig_decl(heap: &Heap, form: Value) -> Option<(Symbol, Sig)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    // `sig` (static only) and `sig!` (also runtime-enforced) declare the same
    // signature as far as the checker is concerned — it reads both.
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    // Only an arrow type-expr is a callable signature worth recording.
    let sig = parse_type(heap, items[2])?.as_arrow()?.clone();
    Some((name, sig))
}

/// If `form` is a `(sig name T)` declaration whose type-expr `T` is a **value
/// type** (not an arrow), return `(name, T)`. The non-function counterpart of
/// [`parse_sig_decl`]: `(sig x int)` declares the *value* `x` has type `int`,
/// which the gradual-assignment check consults to verify `(def x …)`. Returns
/// `None` for a non-`sig` form or an arrow type-expr (that's `parse_sig_decl`'s).
pub(super) fn parse_value_sig_decl(heap: &Heap, form: Value) -> Option<(Symbol, Ty)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    let ty = parse_type(heap, items[2])?;
    // A function arrow is a *callable* signature — that's `parse_sig_decl`'s job;
    // here we only take a plain value type.
    if ty.as_arrow().is_some() {
        return None;
    }
    Some((name, ty))
}
