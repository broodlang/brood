use super::ctx::Ctx;
use super::infer::expr_ty;
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
                    if !value_member_of(heap, it, &elem) {
                        return false;
                    }
                }
            }
            Value::Pair(_) => {
                let mut cur = v;
                while let Value::Pair(p) = cur {
                    let (h, t) = heap.pair(p);
                    if !value_member_of(heap, h, &elem) {
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
        "(filter math/even? [1 2 3 4])",
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
        "(filter math/even? [1 2 3 4])",
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
