//! Step 4 (v0): a small **advisory** type checker — the first consumer of the
//! `Ty`/`GradualTy` lattice, so the type system finally *does* something.
//!
//! It walks a macro-expanded form and warns when a **primitive** is called with
//! an argument that is *provably* the wrong type — e.g. `(first 5)` or
//! `(%add 1 "x")`. "Provably wrong" means the argument's type is **disjoint**
//! from what the primitive accepts (they share no tag). That's the conservative
//! choice that gives **no false positives**: a superset type (`number` where
//! `int` is wanted), an `any` result, or an unknown/`dynamic()` argument all
//! overlap the expected type, so they're never flagged. It never rejects
//! anything — it returns warnings (contract point #5).
//!
//! Scope of v0 (each a later increment):
//! - Only **primitive** calls are checked; `(+ 1 "x")` doesn't warn because `+`
//!   is a Brood closure — closure-signature inference comes later.
//! - Argument types come from **literals** and from the **result type of a
//!   nested primitive call**; variables are `dynamic()`. No flow/guard narrowing
//!   yet (the `Ty::tested_by` bridge is ready for that step).
//! - Primitive signatures live in [`primitive_sig`] here; per contract point #6
//!   they should eventually move onto `NativeFn` (enforced, like `Arity`).

use crate::heap::Heap;
use crate::types::{GradualTy, Ty};
use crate::value::{self, Tag, Value};

/// A primitive's type signature: the expected type of each fixed positional
/// argument, and the result type. (Variadic primitives — `str`, `vector`, the
/// predicates — take `any`, so they have no useful signature and are omitted.)
struct Sig {
    params: Vec<Ty>,
    ret: Ty,
}

/// The signature of a primitive by name, or `None` if it isn't usefully typed
/// (variadic / all-`any` params). The single source of truth for the checker;
/// the values are the discriminating ones — those with non-`any` parameters,
/// where a wrong-typed literal is catchable.
fn primitive_sig(name: &str) -> Option<Sig> {
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let string = Ty::of(Tag::Str);
    let vector = Ty::of(Tag::Vector);
    let any = Ty::ANY;
    // `first`/`rest` accept a list (nil ∪ pair) or a vector.
    let seq = Ty::LIST.union(vector);
    Some(match name {
        "%add" | "%sub" | "%mul" | "%div" => Sig { params: vec![num, num], ret: num },
        "%lt" => Sig { params: vec![num, num], ret: Ty::of(Tag::Bool) },
        "mod" | "rem" => Sig { params: vec![int, int], ret: int },
        "first" | "rest" => Sig { params: vec![seq], ret: any },
        "vector-ref" => Sig { params: vec![vector, int], ret: any },
        "vector-length" => Sig { params: vec![vector], ret: int },
        "string-length" => Sig { params: vec![string], ret: int },
        "substring" => Sig { params: vec![string, int, int], ret: string },
        _ => return None,
    })
}

/// The static type of an expression form, as a [`GradualTy`]. Self-evaluating
/// literals get their exact tag; a variable (bare symbol) or anything we can't
/// pin is `dynamic()`; a primitive call gets that primitive's result type; a
/// `quote`d datum gets the datum's tag.
fn expr_ty(heap: &Heap, form: Value) -> GradualTy {
    match form {
        // A bare symbol in code is a variable reference — unknown.
        Value::Sym(_) => GradualTy::dynamic(),
        Value::Pair(_) => match list_items(heap, form) {
            Some(items) => match items.first().copied() {
                Some(Value::Sym(s)) => {
                    let head = value::symbol_name(s);
                    if head == "quote" {
                        return items
                            .get(1)
                            .map(|&d| GradualTy::stat(Ty::of_value(d)))
                            .unwrap_or_else(GradualTy::dynamic);
                    }
                    match primitive_sig(&head) {
                        Some(sig) => GradualTy::stat(sig.ret),
                        None => GradualTy::dynamic(),
                    }
                }
                _ => GradualTy::dynamic(),
            },
            None => GradualTy::dynamic(),
        },
        // Int / Float / Str / Keyword / Bool / Nil / Vector: self-evaluating.
        other => GradualTy::stat(Ty::of_value(other)),
    }
}

/// Check one macro-expanded form, returning a warning per provable misuse. Empty
/// when nothing is provably wrong (which includes "not enough static info").
pub fn check_form(heap: &Heap, form: Value) -> Vec<String> {
    let mut out = Vec::new();
    check_into(heap, form, &mut out);
    out
}

fn check_into(heap: &Heap, form: Value, out: &mut Vec<String>) {
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else { return };
    let Some(&head) = items.first() else { return };

    if let Value::Sym(s) = head {
        let name = value::symbol_name(s);
        // `quote`/`quasiquote` enclose data, not code — don't look inside.
        if name == "quote" || name == "quasiquote" {
            return;
        }
        if let Some(sig) = primitive_sig(&name) {
            for (i, &param) in sig.params.iter().enumerate() {
                if let Some(&arg) = items.get(i + 1) {
                    // Warn only on *provable* mismatch: the argument's type shares
                    // no tag with what the primitive accepts. A superset, `any`,
                    // or unknown (`dynamic()`, bound `ANY`) argument overlaps the
                    // param, so it's never flagged — no false positives.
                    let arg_ty = expr_ty(heap, arg).bound;
                    if arg_ty.is_disjoint(param) {
                        out.push(format!(
                            "{}: argument {} expects {}, got {} ({})",
                            name,
                            i + 1,
                            param,
                            arg_ty,
                            crate::printer::print(heap, arg),
                        ));
                    }
                }
            }
        }
    }

    // Recurse: arguments (and any nested forms) may themselves be calls.
    for &item in &items {
        check_into(heap, item, out);
    }
}

/// The elements of a proper list, or `None` for an improper list / non-list.
fn list_items(heap: &Heap, mut v: Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    loop {
        match v {
            Value::Nil => return Some(out),
            Value::Pair(p) => {
                let (head, tail) = heap.pair(p);
                out.push(head);
                v = tail;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader;

    fn warnings(src: &str) -> Vec<String> {
        let mut heap = Heap::new();
        let form = reader::read_one(&mut heap, src).expect("parse");
        check_form(&heap, form)
    }

    #[test]
    fn flags_literal_misuse_of_primitives() {
        assert!(warnings("(first 5)").iter().any(|w| w.contains("first") && w.contains("int")));
        assert!(warnings("(string-length :k)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("keyword")));
        assert!(warnings("(%add 1 \"x\")").iter().any(|w| w.contains("%add")));
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
    fn closures_are_not_checked_yet() {
        // `+` is a Brood fn, not a primitive — no signature, so no warning (yet).
        assert!(warnings("(+ 1 \"x\")").is_empty());
    }

    #[test]
    fn covers_the_other_signed_primitives() {
        assert!(warnings("(mod 7 3)").is_empty());
        assert!(warnings("(mod 7 \"x\")").iter().any(|w| w.contains("mod")));
        assert!(warnings("(rem :a 3)").iter().any(|w| w.contains("rem")));
        assert!(warnings("(vector-length 5)").iter().any(|w| w.contains("vector-length")));
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
        assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol
        assert!(warnings("(first)").is_empty()); // missing arg → nothing to check
    }
}
