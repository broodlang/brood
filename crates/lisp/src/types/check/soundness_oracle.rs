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
    // Map refinements. Without these the oracle passes on any map-typed expression
    // whatever the refinement claims — which is how `(assoc m :extra "text")` on a
    // `(map keyword int)` went on reporting `(map keyword int)`, flagging correct code
    // downstream. A rule that carries a refinement through an operation that CHANGES it
    // is invisible to a tags-only check.
    if let Value::Map(id) = v {
        let entries = heap.map_entries(id);
        if let Some((key_ty, val_ty)) = ty.map_kv() {
            for (k, val) in &entries {
                if !value_member_of(heap, *k, key_ty) || !value_member_of(heap, *val, val_ty) {
                    return false;
                }
            }
        }
        if let Some(fields) = ty.record_fields() {
            for (name, (field_ty, required)) in fields {
                match entries
                    .iter()
                    .find(|(k, _)| matches!(k, Value::Keyword(n) if n == name))
                {
                    Some((_, held)) => {
                        if !value_member_of(heap, *held, field_ty) {
                            return false;
                        }
                    }
                    // A required field the value does not carry.
                    None if *required => return false,
                    None => {}
                }
            }
            // A CLOSED record (ADR-264) declares that no other key is present.
            if ty.record_is_open() == Some(false)
                && entries
                    .iter()
                    .any(|(k, _)| !matches!(k, Value::Keyword(n) if fields.contains_key(n)))
            {
                return false;
            }
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
        // maps and record shapes — the refinement-carrying rules. `assoc` is the one
        // that was wrong: it kept `K`/`V` unchanged whatever it added.
        "{:a 1 :b 2}",
        "{:a 1 :b \"two\"}",
        "{}",
        "(assoc {:a 1} :b 2)",
        "(assoc {:a 1} :b \"text\")",
        "(assoc {:a 1} :a \"replaced\")",
        "(dissoc {:a 1 :b 2} :b)",
        "(keys {:a 1 :b 2})",
        "(vals {:a 1 :b 2})",
        "(get {:a 1} :a)",
        "(get {:a 1} :missing)",
        "(assoc (assoc {} :a 1) :b :k)",
        // sequence rules that reshape rather than construct — each one carries an
        // element refinement through an operation, the shape ADR-269 showed is where
        // an under-approximation hides.
        "(reverse [1 2 3])",
        "(rest [1 2 3])",
        "(rest [1])",
        "(seq/but-last [1 2 3])",
        "(seq/distinct [1 1 2])",
        "(sort [3 1 2])",
        "(seq/sort-by (fn (x) x) [3 1 2])",
        "(take 2 [1 2 3])",
        "(drop 2 [1 2 3])",
        "(take 0 [1 2 3])",
        "(drop 5 [1 2 3])",
        "(seq/take-while math/even? [2 4 5])",
        "(seq/drop-while math/even? [2 4 5])",
        "(seq/remove math/even? [1 2 3])",
        "(cons 1 [2 3])",
        "(cons \"a\" [2 3])",
        "(append [1 2] [\"a\"])",
        "(append)",
        "(range 3)",
        "(range 1 5)",
        "(seq/keep (fn (x) (if (math/even? x) x nil)) [1 2 3 4])",
        "(seq/interpose 0 [1 2 3])",
        // mixed-element and nested collections
        "[[1 2] [3]]",
        "[{:a 1} {:b 2}]",
        "(map (fn (x) [x x]) [1 2])",
        "(first [[1 2] [3]])",
        // strings and numbers
        "(str 1 \"a\" :k)",
        "(string/upcase \"a\")",
        "(string/split \"a,b\" \",\")",
        "(math/abs -3)",
        "(math/quot 7 2)",
        "(/ 7 2)",
        "(+ 1 2.5)",
        // guards / branches — a union of both arms
        "(if true 1 \"a\")",
        "(if false 1 \"a\")",
        "(let (x 5) (if (math/even? x) x nil))",
    ];
    let mut claimed = 0usize;
    for src in cases {
        let mut interp = crate::Interp::new();
        // Static type of the form (read in this heap, typed in the empty ctx).
        let form = crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse");
        let Some(t) = expr_ty(&interp.heap, form, &Ctx::default()) else {
            continue; // checker makes no claim → nothing to verify
        };
        claimed += 1;
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
    // A case the checker declines to type is skipped, so a corpus that mostly declines
    // would pass while verifying nothing. Assert the coverage instead of assuming it:
    // this is the difference between "the oracle found nothing" and "the oracle ran".
    // It types 77 of 82 today; the bar is 90%, so a handful of deliberately untypeable
    // cases is fine and a collapse is not.
    assert!(
        claimed * 10 >= cases.len() * 9,
        "the oracle verified only {claimed} of {} cases — it is not covering what it \
         looks like it covers",
        cases.len()
    );
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
        // Maps and records — the refinement-carrying rules (ADR-269). `assoc` adding a
        // value outside the map's value type is the one that WAS a false positive.
        "(get {:a 1} :a)",
        "(+ 1 (get {:a 1} :a))",
        "(string/length (get (assoc {:a 1} :b \"text\") :b))",
        "(string/length (get (assoc {:a 1} :a \"replaced\") :a))",
        "(+ 1 (get (dissoc {:a 1 :b 2} :b) :a))",
        "(map (fn (k) k) (keys {:a 1 :b 2}))",
        "(map (fn (v) v) (vals {:a 1 :b 2}))",
        "(let (r {:name \"x\"}) (string/length (get r :name)))",
        "(let (r (assoc {} :name \"x\")) (string/length (get r :name)))",
        // Literal narrowing on BOTH branches (ADR-268) — the else branch now knows what
        // the equality test ruled out, and must not over-claim on the way.
        "(let (tag :ok) (if (%eq tag :ok) 1 (string/length tag)))",
        "(let (tag :err) (if (%eq tag :ok) 1 :other))",
        "(let (n 5) (if (%eq n 5) (+ n 1) (+ n 2)))",
        "(let (s \"x\") (if (%eq s \"x\") 1 (string/length s)))",
        "(let (b true) (if b 1 2))",
        "(let (x nil) (if (%eq x nil) 0 (+ x 1)))",
        // Multi-alternative unions reaching a call (ADR-267's per-tag decomposition).
        "(let (v (if true 1 [2])) (if (vector? v) (first v) v))",
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
const PROBE_ARGS: [&str; 11] = [
    "5",
    "1.5",
    "\"s\"",
    ":k",
    "nil",
    "true",
    "[1 2]",
    "(list 1 2)",
    "{:a 1}",
    // A callable, in both spellings the runtime accepts — without these, a domain that
    // wrongly excludes functions is invisible here, since a probe that raises is skipped
    // and every non-callable probe raises against a body that calls its parameter.
    "inc",
    "(fn (x) x)",
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
        // Equality guards, whose ELSE branch narrows now that a literal complement is
        // exact (ADR-268). The domain must still contain every value `f` accepts — an
        // else-branch that over-narrows would exclude one and warn on a working call.
        "(defn f (x) (if (%eq x :ok) 1 2))",
        "(defn f (x) (if (%eq x :ok) 1 (str x)))",
        "(defn f (x) (if (%eq x 5) (+ x 1) 0))",
        "(defn f (x) (if (%eq x nil) 0 (str x)))",
        "(defn f (x) (cond (%eq x :ok) 1 (%eq x :err) 2 else 3))",
        "(defn f (x) (if (%eq x :ok) 1 (if (%eq x :err) 2 3)))",
        // Record/map shapes reaching a parameter — the sinks carry a shape forward now.
        "(defn f (x) (get (assoc x :k 1) :k))",
        // A parameter in call-head position — the callback shape. Its domain must
        // contain every callable, and nothing that isn't one is passed here.
        "(defn f (g) (g 1))",
        "(defn f (g) (+ 1 (g 1)))",
        "(defn f (g x) (g x))",
        "(defn f (g) (map g [1 2 3]))",
        // A map argument, so the `:keyword` probe actually succeeds — a keyword is a
        // function OF A MAP, and a domain that admits only `fn | native` is caught here
        // and nowhere else (every other probe raises, and a raising probe is skipped).
        "(defn f (g) (g {:a 1}))",
        "(defn f (x) (count (keys x)))",
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
