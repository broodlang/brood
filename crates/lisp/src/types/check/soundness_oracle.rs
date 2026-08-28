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
        "(string/length \"hi\")",
        "(+ 1 2)",
        "(- 10 3 2)",
        "(* 2 3)",
        "(< 1 2)",
        "(<= 1 1)",
        "(string/->number \"5\")",
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
        "(string/length (str 1 2 3))",
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

/// The argument values every domain case is probed with — one per tag family the
/// lattice distinguishes, so a domain that wrongly excludes any of them is caught.
const PROBE_ARGS: [&str; 9] = [
    "5",
    "1.5",
    "\"s\"",
    ":k",
    "nil",
    "true",
    "[1 2]",
    "(list 1 2)",
    "{:a 1}",
];

#[test]
fn an_inferred_parameter_domain_over_approximates_the_real_one() {
    // Facet (III), for ADR-261: a parameter's inferred **domain** must contain every
    // value the function actually accepts. The runtime is the oracle — if a call
    // succeeds, that argument was in the true domain, so a domain excluding it would
    // make the checker warn on a working call.
    //
    // The cases are the shapes ADR-261 introduced (branch unions, guard slices,
    // `match` patterns, destructuring, `cond`, short-circuits, `let` aliases), where
    // an *under*-approximation would hide.
    let cases = [
        "(defn f (x) (if (string? x) (string/length x) (+ x 1)))",
        "(defn f (x) (if (nil? x) 0 (+ x 1)))",
        "(defn f (x) (if (string? x) 1 2))",
        "(defn f (x) (when (string? x) (string/length x)))",
        "(defn f (x) (unless (string? x) 0))",
        "(defn f (x) (cond (nil? x) 0 (string? x) (string/length x) else 1))",
        "(defn f (x) (and (string? x) (string/length x)))",
        "(defn f (x) (or (nil? x) (+ x 1)))",
        "(defn f (x) (match x ((:ok v) v) (_ 0)))",
        "(defn f (x) (let (y x) (if (int? y) (+ y 1) 0)))",
        "(defn f (x) (do (str x) (if (int? x) (+ x 1) 0)))",
        "(defn f (x) (if (and (int? x) (> x 0)) (+ x 1) 0))",
        "(defn f (x) (str x))",
        "(defn f (x) (+ x 1))",
        "(defn f (x) x)",
        // Truthiness (ADR-264's tail): a bare local as an `if` test is a guard, and it
        // is one-sided because the exact truthy type is unsayable here. These are the
        // shapes that guard fires on — including the `if-let` expansion that motivated
        // it, where a closed literal's `nil` had read as a false positive.
        "(defn f (x) (if x (inc x) 0))",
        "(defn f (x) (if x 1 (string/length x)))",
        "(defn f (x) (let (v x) (if v (inc v) 0)))",
        "(defn f (x) (when x (inc x)))",
        "(defn f (x) (unless x (string/length x)))",
        "(defn f (x) (if-let (v (get x :k)) (inc v) 0))",
        "(defn f (x) (when-let (v (get x :k)) (str v)))",
        "(defn f (x) (if (not x) 0 (inc x)))",
    ];
    for def in cases {
        for arg in PROBE_ARGS {
            let mut interp = crate::Interp::new();
            interp.eval_str(def).expect("def");
            let call = format!("(f {arg})");
            // Only a call that RUNS makes a claim: the value was in the true domain.
            if interp.eval_str(&call).is_err() {
                continue;
            }
            // The inference memo is a *thread-local* cleared per `check_file`; this
            // oracle calls `sig_of` directly across many fresh images, so it must clear
            // it itself or case N reads case N-1's answer (which is how this test first
            // "found" an unsoundness that was its own).
            super::sigs::clear_sig_memo();
            let Some(sig) = super::sigs::sig_of(&interp.heap, value::intern("f")) else {
                continue; // no inferred signature → no claim
            };
            let Some(domain) = sig.params.first() else {
                continue; // params-less (return-only) sig → no claim
            };
            let v = interp.eval_str(arg).expect("arg evaluates");
            assert!(
                value_member_of(&interp.heap, v, domain),
                "UNSOUND DOMAIN: `{def}` accepts {arg} at run time, but its inferred \
                 parameter domain is `{domain}` — the checker would warn on a working call",
            );
        }
    }
}

#[test]
fn an_inferred_clause_domain_over_approximates_the_real_one() {
    // The same claim for a multi-arm definition, where the domain is per clause and a
    // call is ruled out only if EVERY arity-relevant arm rejects it (ADR-261). A value
    // the runtime accepts must be admitted by at least one arm of that arity.
    let cases = [
        "(defn f ((x) :when (string? x) (string/length x)) ((x) :when (int? x) (+ x 1)))",
        "(defn f ((x) :when (string? x) 1) ((x) 2))",
        "(defn f ((x) (str x)) ((x y) (str x y)))",
        "(defn f ((x) :when (nil? x) 0) ((x) :when (vector? x) (count x)) ((x) 9))",
    ];
    for def in cases {
        for arg in PROBE_ARGS {
            let mut interp = crate::Interp::new();
            interp.eval_str(def).expect("def");
            if interp.eval_str(&format!("(f {arg})")).is_err() {
                continue;
            }
            super::sigs::clear_sig_memo(); // see the note in the sibling test
            let Some(arms) = super::sigs::infer_overload_of(&interp.heap, value::intern("f"))
            else {
                continue;
            };
            let v = interp.eval_str(arg).expect("arg evaluates");
            let admitted = arms.iter().filter(|s| s.params.len() == 1).any(|s| {
                s.params
                    .first()
                    .is_none_or(|d| value_member_of(&interp.heap, v, d))
            });
            assert!(
                admitted,
                "UNSOUND CLAUSE DOMAIN: `{def}` accepts {arg} at run time, but no \
                 1-argument clause's inferred domain admits it: {:?}",
                arms.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            );
        }
    }
}
