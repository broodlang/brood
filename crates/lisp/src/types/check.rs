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
//! Signatures come from two tables: primitives ([`primitive_sig`]) and curated
//! stdlib closures ([`curated_sig`] — `+`, `<`, `map`, …). Argument types come
//! from literals and from a nested call's result type; a variable is unknown
//! (`None`) and never flagged.
//!
//! Vocabulary is `Option<Ty>` (known / unknown), not `GradualTy` — the
//! disjointness check only needs "do I know this type?". Forms inside `try` /
//! `error-of` / `assert-error` are skipped (they deliberately exercise failures).
//!
//! Not yet (later increments): inferring signatures from a fn body, guard
//! narrowing (`Ty::tested_by` is prepped), and running automatically in
//! `brood check`/`run`/`test`. Today the entry point is the `(check 'form)`
//! builtin.

use crate::core::heap::Heap;
use crate::core::value::{self, Tag, Value};
use crate::error::Pos;
use crate::types::Ty;

/// A callee's type signature: the expected type of each fixed positional
/// argument, an optional type for variadic trailing args (`rest`), and the
/// result type.
struct Sig {
    params: Vec<Ty>,
    rest: Option<Ty>,
    ret: Ty,
}

impl Sig {
    /// The type expected at argument position `i` — the fixed params, then
    /// `rest` for anything beyond. `None` means "accepts anything here".
    fn param(&self, i: usize) -> Option<Ty> {
        self.params.get(i).copied().or(self.rest)
    }
}

/// The signature of a **primitive** by name, or `None` if it isn't usefully
/// typed (variadic / all-`any` params). The discriminating ones — those with
/// non-`any` parameters, where a wrong-typed argument is catchable.
fn primitive_sig(name: &str) -> Option<Sig> {
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let string = Ty::of(Tag::Str);
    let vector = Ty::of(Tag::Vector);
    let any = Ty::ANY;
    // `first`/`rest` accept a list (nil ∪ pair) or a vector.
    let seq = Ty::LIST.union(vector);
    let s = |params: Vec<Ty>, ret: Ty| Sig {
        params,
        rest: None,
        ret,
    };
    Some(match name {
        "%add" | "%sub" | "%mul" | "%div" => s(vec![num, num], num),
        "%lt" => s(vec![num, num], Ty::of(Tag::Bool)),
        "rem" => s(vec![int, int], int),
        "first" | "rest" => s(vec![seq], any),
        "vector-ref" => s(vec![vector, int], any),
        "vector-length" => s(vec![vector], int),
        "string-length" => s(vec![string], int),
        "substring" => s(vec![string, int, int], string),
        _ => return None,
    })
}

/// Signatures for the stable stdlib **closures** the checker can't infer but
/// that matter: the arithmetic/comparison kernel (variadic over numbers) and the
/// core higher-order fns. Hand-vetted, so sound — this is what makes `(+ 1 "x")`
/// catchable even though `+` is `(reduce %add 0 xs)`.
fn curated_sig(name: &str) -> Option<Sig> {
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let any = Ty::ANY;
    let seq = Ty::LIST.union(Ty::of(Tag::Vector));
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));
    Some(match name {
        // variadic arithmetic: every argument must be a number
        "+" | "-" | "*" | "/" => Sig {
            params: vec![],
            rest: Some(num),
            ret: num,
        },
        // variadic comparison: numeric args, boolean result
        "<" | "<=" | ">" | ">=" => Sig {
            params: vec![],
            rest: Some(num),
            ret: Ty::of(Tag::Bool),
        },
        // `mod` is Brood (over `rem`), but its types are fixed
        "mod" => Sig {
            params: vec![int, int],
            rest: None,
            ret: int,
        },
        // higher-order: first arg callable, second a sequence
        "map" | "filter" => Sig {
            params: vec![callable, seq],
            rest: None,
            ret: seq,
        },
        "reduce" => Sig {
            params: vec![callable, any, seq],
            rest: None,
            ret: any,
        },
        _ => return None,
    })
}

/// The signature for `name`, from either table (primitive, then curated stdlib).
fn sig_of(name: &str) -> Option<Sig> {
    primitive_sig(name).or_else(|| curated_sig(name))
}

/// The static type of an expression form, or `None` when it can't be pinned (a
/// variable, or a call whose callee has no known signature). `None` is "unknown"
/// and is never flagged. Self-evaluating literals get their exact tag; a `quote`d
/// datum gets the datum's tag; a call with a known signature gets its result type.
fn expr_ty(heap: &Heap, form: Value) -> Option<Ty> {
    match form {
        // A bare symbol in code is a variable reference — unknown.
        Value::Sym(_) => None,
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            match items.first().copied() {
                Some(Value::Sym(s)) => {
                    let head = value::symbol_name(s);
                    if head == "quote" {
                        return items.get(1).map(|&d| Ty::of_value(d));
                    }
                    sig_of(&head).map(|sig| sig.ret)
                }
                _ => None,
            }
        }
        // Int / Float / Str / Keyword / Bool / Nil / Vector: self-evaluating.
        other => Some(Ty::of_value(other)),
    }
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
    check_into(heap, form, &mut out);
    out
}

/// Forms whose contents are data (`quote`/`quasiquote`) or deliberately exercise
/// failures (`try` and the error-asserting test helpers it expands from) — don't
/// look inside them.
fn skips_body(name: &str) -> bool {
    matches!(
        name,
        "quote" | "quasiquote" | "try" | "error-of" | "assert-error"
    )
}

fn check_into(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&head) = items.first() else { return };

    if let Value::Sym(s) = head {
        let name = value::symbol_name(s);
        if skips_body(&name) {
            return;
        }
        if let Some(sig) = sig_of(&name) {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Warn only on a *provable* mismatch: the argument's type shares
                // no tag with what the callee accepts. A superset, an `any`
                // result, or an unknown argument (`None`) overlaps the param, so
                // it's never flagged — no false positives.
                if let Some(arg_ty) = expr_ty(heap, arg) {
                    if arg_ty.is_disjoint(param) {
                        let msg = format!(
                            "{}: argument {} expects {}, got {} ({})",
                            name,
                            i + 1,
                            param,
                            arg_ty,
                            crate::syntax::printer::print(heap, arg),
                        );
                        // Locate to the call form (a Pair the reader positioned).
                        out.push((heap.form_pos(form), msg));
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
    use crate::syntax::reader;

    fn warnings(src: &str) -> Vec<String> {
        let mut heap = Heap::new();
        let form = reader::read_one(&mut heap, src).expect("parse");
        check_form(&heap, form)
    }

    #[test]
    fn flags_literal_misuse_of_primitives() {
        assert!(warnings("(first 5)")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
        assert!(warnings("(string-length :k)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("keyword")));
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
        assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol
        assert!(warnings("(first)").is_empty()); // missing arg → nothing to check
    }
}
