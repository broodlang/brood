//! `(sig name (params… -> ret))` type annotations — the parser from a Brood
//! type-expression *form* to a [`Ty`]/[`Sig`], plus the recogniser that pulls a
//! declaration out of a top-level form. See `docs/type-annotations.md`.
//!
//! Slice 1 is **checker-facing only**: a declared `Sig` is read by the checker as
//! an authoritative signature source (ahead of primitive / curated / inferred).
//! The `sig` form is a runtime no-op (a prelude macro expanding to `nil`), so the
//! scan runs over the *un-expanded* forms — like the hygiene lint. Nothing here
//! enforces the declaration at run time yet; that is slice 2 (the strong arrow).

use crate::core::heap::Heap;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

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
            // (map K V) — key/value typed map.  Slice 1: parse and validate K/V
            // annotations but produce a flat Ty::Map for the checker (refinement
            // tracking deferred to slice 2 when a real consumer drives it).
            if value::symbol_is(head, "map") && items.len() == 3 {
                parse_type(heap, items[1])?;
                parse_type(heap, items[2])?;
                return Some(Ty::of(Tag::Map));
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
