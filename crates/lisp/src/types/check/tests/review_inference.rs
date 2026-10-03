//! Regressions from the 2026-10-02 type-system review: inference rules, narrowing, the
//! gradual relation, signature sources and the soundness oracle.

use super::*;

// ---- 1: a LOADED closure's self-call is typed by an ascent, not as ⊥ ----------------

#[test]
fn loaded_recursive_list_builder_is_not_typed_nil() {
    // Single-pass inference used to type `(h (- n 1))` as ⊥ and keep the first round,
    // so `h` was `-> nil` and `(nth (h 3) 2)` read `nil`.
    let w = check_with_defs(
        &["(defn rv-h (n) (if (= n 0) nil (cons n (rv-h (- n 1)))))"],
        "(+ 1 (nth (rv-h 3) 2))",
    );
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn loaded_self_call_in_a_when_is_not_a_diverging_guard() {
    // `(when (nil? x) (dv 0))` falls through when `dv` returns — which it does.
    let w = check_with_defs(
        &["(defn rv-dv (x) (do (when (nil? x) (rv-dv 0)) (if (nil? x) \"none\" (+ x 1))))"],
        "(string/length (rv-dv nil))",
    );
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn loaded_multi_arity_self_call_reaches_the_ascent() {
    let w = check_with_defs(
        &["(defn rv-mh ((n) (if (= n 0) nil (cons n (rv-mh (- n 1))))) ((n m) nil))"],
        "(+ 1 (nth (rv-mh 3) 2))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 2: `merge` with an OPEN later argument may override an earlier field ------------

#[test]
fn merge_with_an_open_later_argument_forgets_the_earlier_field_type() {
    let w = file_warnings(
        "(sig cfg ((record &open :host string) -> int))\n\
         (defn cfg (opts) (string/length (:port (merge {:port 80} opts))))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 3: `cons` onto a vector is an improper pair; its `rest` is the vector -----------

#[test]
fn cons_onto_a_vector_is_not_a_list() {
    let w = file_warnings("(def c (cons 1 [2 3]))\n(defn f () (seq/vector-length (rest c)))");
    assert!(w.is_empty(), "{w:?}");
}

// ---- 4: `seq` passes a vector or string through unchanged ----------------------------

#[test]
fn seq_of_a_vector_or_string_is_the_input() {
    let w = file_warnings(
        "(defn f () (seq/vector-length (seq [1 2 3])))\n\
         (defn g () (string/length (seq \"abc\")))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 5: `range` with a step may descend ---------------------------------------------

#[test]
fn a_descending_range_is_not_empty() {
    let w = file_warnings("(defn f () (+ 1 (first (range 5 2 -1))))");
    assert!(w.is_empty(), "{w:?}");
}

// ---- 6: `count` of a defrecord value does not count `:__id__` -----------------------

#[test]
fn record_count_hides_the_identity_key() {
    let w = file_warnings(
        "(defrecord rv-pt (x y))\n\
         (sig two ((int 2 2) -> int))\n\
         (defn two (n) n)\n\
         (defn f () (two (count (rv-pt 1 2))))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 7: rebinding a guard alias's TARGET drops the alias -----------------------------

#[test]
fn guard_alias_dies_with_its_target_binding() {
    let w = file_warnings(
        "(defn f (x)\n\
           (let (ok (int? x))\n\
             (let (x \"s\")\n\
               (if ok (string/length x) (string/length x)))))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 8: int-closed arithmetic over a dynamic operand is not precise ------------------

#[test]
fn arithmetic_over_a_call_result_is_checked_by_overlap() {
    let w = file_warnings(
        "(sig idx ((int 0 _) -> int))\n\
         (defn idx (i) i)\n\
         (defn f (m) (let (n (count (keys m))) (idx (- n 1))))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 9: `nth` with a default answers the default past the end ------------------------

#[test]
fn nth_default_past_a_known_end_is_the_default() {
    let w = file_warnings(
        "(defn a () (string/length (nth [\"a\"] 1 \"\")))\n\
         (defn b () (+ (nth [1] 1 0) 1))\n\
         (defn c () (string/length (get [\"a\"] 1 \"\")))",
    );
    assert!(w.is_empty(), "{w:?}");
}

// ---- 12: a field's truthiness selects among closed record alternatives (strict) ------

#[test]
fn field_truthiness_narrows_a_closed_record_union() {
    let src = "(sig e ((or (record :ok int) (record :error string)) -> int))\n\
               (defn e (r) (if (:ok r) (+ (:ok r) 1) (string/length (:error r))))\n\
               (sig e2 ((or (record :ok int) (record :error string)) -> int))\n\
               (defn e2 (r) (let (v (:ok r)) (if v (+ v 1) (string/length (:error r)))))\n\
               (sig e3 ((or (record :ok int) (record :error string)) -> int))\n\
               (defn e3 (r) (if (contains? r :ok) (+ (:ok r) 1) (string/length (:error r))))";
    let w = file_warnings_mode(src, true);
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn field_truthiness_keeps_an_alternative_whose_field_may_be_falsy() {
    // `:ok` admits `nil`, so a falsy `(:ok r)` does not rule the `:ok` record out.
    let src = "(sig e ((or (record :ok (or nil int)) (record :error string)) -> int))\n\
               (defn e (r) (if (:ok r) 1 (string/length (:error r))))";
    let w = file_warnings_mode(src, true);
    assert!(w.iter().any(|m| m.contains("string/length")), "{w:?}");
}

// ---- 14: an `&optional` parameter's domain constrains a passed argument --------------

#[test]
fn optional_parameter_domain_is_checked_at_a_call() {
    let w = file_warnings(
        "(defn h (a &optional (b \"x\")) (if (string? b) (string/length b) (+ a b)))\n\
         (defn g () (h 1 :kw))",
    );
    assert!(w.iter().any(|m| m.contains("h: argument 2")), "{w:?}");
    let ok = file_warnings(
        "(defn h (a &optional (b \"x\")) (if (string? b) (string/length b) (+ a b)))\n\
         (defn g () (list (h 1) (h 1 2) (h 1 \"s\")))",
    );
    assert!(ok.is_empty(), "{ok:?}");
}

// ---- 16: a quoted list that misses a declared uniform list is reported ----------------

// (Fixed in the lattice, not by a rule here: a positional list shape that misses a uniform
// `list<int>` at any position is disjoint from it, so the overlap reading catches it too.)
#[test]
fn quoted_list_against_a_uniform_list_is_checked() {
    let w = file_warnings(
        "(sig li ((list int) -> int))\n(defn li (xs) 1)\n(defn f () (li '(1 \"x\")))",
    );
    assert!(w.iter().any(|m| m.contains("li: argument 1")), "{w:?}");
    // …and a long one, past any positional-shape width.
    let long: Vec<String> = (1..=40).map(|n| n.to_string()).collect();
    let w = file_warnings(&format!(
        "(sig li ((list int) -> int))\n(defn li (xs) 1)\n(defn f () (li '({} \"x\")))",
        long.join(" ")
    ));
    assert!(w.iter().any(|m| m.contains("li: argument 1")), "{w:?}");
    let ok =
        file_warnings("(sig li ((list int) -> int))\n(defn li (xs) 1)\n(defn f () (li '(1 2)))");
    assert!(ok.is_empty(), "{ok:?}");
}

// ---- 17: rule heads are spellings that are bound -------------------------------------

#[test]
fn dedupe_does_not_carry_the_input_length() {
    let w = file_warnings(
        "(sig two ((int 2 2) -> int))\n(defn two (n) n)\n\
         (defn f () (two (count (seq/dedupe [1 1 2]))))",
    );
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn trigonometry_is_always_float() {
    let w = file_warnings(
        "(sig want-int (int -> int))\n(defn want-int (n) n)\n(defn f () (want-int (math/sin 1)))",
    );
    assert!(w.iter().any(|m| m.contains("got float")), "{w:?}");
}

/// Every name a refinement rule keys on (`symbol_is(_, "name")` in the rule files) must be
/// a name that is BOUND — a special form, a prelude global, a module function — or one of
/// the non-callee keys listed below. A rename wave that moves a function leaves the rule
/// keyed on the old spelling, and the rule then dies silently (2026-10-02: bare
/// `distinct`, `sin`/`cos`/`tan`, `quot`/`rem`/`mod`, `vector-length`, `print`/`println`/
/// `format` were all dead).
#[test]
fn rule_heads_are_bound() {
    const SOURCES: &[&str] = &[
        include_str!("../infer.rs"),
        include_str!("../guards.rs"),
        include_str!("../sigs.rs"),
        include_str!("../walk/calls.rs"),
        include_str!("../walk/binders.rs"),
        include_str!("../ctx.rs"),
        include_str!("../std_index.rs"),
        include_str!("../../check.rs"),
    ];
    // Keys that are not callees: defmodule clause words, check-allow categories, type
    // constructors, a special-form spelling the reader keeps, and the `contract` module's
    // private shim heads (read off an expanded shim body, never called by user code).
    const NOT_CALLEES: &[&str] = &[
        "alias",
        "as",
        "exclude",
        "only",
        "use",
        "use-internals",
        "generated",
        "duplicate-def",
        "record",
        "tuple",
        "let*",
        "fn",
        "quote",
        "load",
        "%pin",
        "%contract-check-args",
        "%contract-check-rest",
    ];
    let mut names: Vec<String> = Vec::new();
    for source in SOURCES {
        let mut rest = *source;
        while let Some(at) = rest.find("symbol_is(") {
            rest = &rest[at + "symbol_is(".len()..];
            let Some(close) = rest.find(')') else { break };
            let call = &rest[..close];
            if let Some((_, quoted)) = call.split_once(", \"") {
                if let Some(name) = quoted.strip_suffix('"') {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    assert!(
        names.len() > 50,
        "the scan found too few rule heads: {names:?}"
    );
    let mut interp = crate::Interp::new();
    let mut unbound = Vec::new();
    for name in &names {
        if NOT_CALLEES.contains(&name.as_str()) {
            continue;
        }
        if let Some((module, _)) = name.split_once('/').filter(|(m, _)| !m.is_empty()) {
            let _ = interp.eval_str(&format!("(require-one '{module})"));
        }
        let probe = format!(
            "(or (bound? '{name}) (seq/find (reflect/special-forms) (fn (x) (= x '{name}))))"
        );
        let bound = interp
            .eval_str(&probe)
            .map(crate::eval::truthy)
            .unwrap_or(false);
        if !bound {
            unbound.push(name.clone());
        }
    }
    assert!(
        unbound.is_empty(),
        "rule heads keyed on unbound names: {unbound:?}"
    );
}

// ---- coordinator: a same-file `number` function called with ints -------------------

#[test]
fn a_same_file_number_function_specializes_under_int_arguments() {
    // `core.blsp` defines `inc` itself, so the by-name integer rule stands aside there and
    // `(inc slash)` read the inferred `(number) -> number` — under strict, a `number` into
    // `string/substring`'s `int` index.
    // A float caller keeps `my-inc`'s inferred sig at `(number) -> number`, as `inc`'s many
    // callers do in the prelude.
    let src = "(defn my-inc (n) (+ n 1))\n\
               (defn g () (my-inc 1.5))\n\
               (defn f (s) (let (slash (%str-last-index-of s \"/\")) \
                 (if (%eq slash -1) s (string/substring s (my-inc slash) 3))))";
    let w = file_warnings_mode(src, true);
    assert!(w.is_empty(), "{w:?}");
    // …and spelled as the prelude spells it, over the binary kernel primitive.
    let w = file_warnings_mode(&src.replace("(+ n 1)", "(%add n 1)"), true);
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn seq_of_a_union_answers_kind_by_kind() {
    // `(seq x)` over `nil | bytes` is `nil` or a list of octets: the element is an octet,
    // so `(math/rem b 4)` indexes inside the tuple. Declining the whole union (the bytes
    // half converts, the nil half passes through) left `b` unknown and `nth` reading `nil`
    // past an unknown index — `std/encoding`'s hex loop under strict.
    let src = "(sig f ((or nil bytes) -> int))\n\
               (defn f (bs) (let (b (first (seq bs))) \
                 (if (nil? b) 0 (+ 1 (nth [1 2 3 4] (math/rem b 4))))))";
    let w = file_warnings_mode(src, true);
    assert!(w.is_empty(), "{w:?}");
}

// ---- 11: a lambda passed to an arrow parameter is typed under that arrow (strict) ------

#[test]
fn a_lambda_is_typed_under_the_arrow_it_is_passed_as() {
    // `(fn (x) (+ x 1))` alone is `(any) -> number`; under `x : int` it returns an int, so
    // strict inclusion against `(int) -> int` holds. A lambda that really returns the wrong
    // thing is still reported.
    let declared = "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n";
    let w = file_warnings_mode(&format!("{declared}(g (fn (x) (+ x 1)))"), true);
    assert!(w.is_empty(), "{w:?}");
    let w = file_warnings_mode(&format!("{declared}(g (fn (x) (str x)))"), true);
    assert!(w.iter().any(|m| m.contains("got (int) -> string")), "{w:?}");
}

// ---- 13: an immediately-applied lambda checks its arguments and arity ----------------

#[test]
fn an_immediately_applied_lambda_checks_its_arguments_and_arity() {
    let w = file_warnings("(defn f () ((fn (y) (string/length y)) 5))");
    assert!(
        w.iter().any(|m| m.contains("argument 1 expects string")),
        "{w:?}"
    );
    let w = file_warnings("(defn f () ((fn (y) y) 1 2))");
    assert!(
        w.iter().any(|m| m.contains("expected 1 argument, got 2")),
        "{w:?}"
    );
    let w = file_warnings("(defn f () ((fn (y) (string/length y)) \"ok\"))");
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn a_deliberate_arity_error_opts_out_under_type_mismatch() {
    // `try` bodies are checked, so a test provoking an arity error on purpose needs a way
    // to say so — for a named callee and for an immediately-applied lambda alike.
    let w = file_warnings(
        "(defn one (x) x)\n\
         (defn a () (check-allow :type-mismatch (try (one 1 2) false (catch _ true))))\n\
         (defn b () (check-allow :type-mismatch (try ((fn (x) x) 1 2) false (catch _ true))))",
    );
    assert!(w.is_empty(), "{w:?}");
    let w = file_warnings("(defn one (x) x)\n(defn c () (try (one 1 2) false (catch _ true)))");
    assert!(
        w.iter().any(|m| m.contains("expected 1 argument, got 2")),
        "{w:?}"
    );
}
