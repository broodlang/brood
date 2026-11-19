//! Inference read off function types, both ways (ADR-725): a parameter no argument can
//! satisfy, predicates inferred as type guards, demands propagated backward through an
//! overloaded callee ("reverse arrows"), narrowing through a `let`-bound test, type-based
//! `match` redundancy, and refinement through a field read and a key test.

use super::*;

fn has(ws: &[String], needle: &str) -> bool {
    ws.iter().any(|warning| warning.contains(needle))
}

// ---- a parameter whose uses conflict is reported at the definition ----

#[test]
fn a_parameter_no_argument_satisfies_is_reported_at_the_definition() {
    // The uses demand a number and a string: no value is both, so every call fails —
    // reported once, here, with no caller in the file.
    let ws = file_warnings("(defn b1 (x) (do (+ x 1) (string/upper x)))");
    assert!(
        has(&ws, "b1: no argument satisfies every use of `x`"),
        "{ws:?}"
    );
}

#[test]
fn guarded_uses_are_alternatives_not_a_conflict() {
    // Each branch demands a different type, but only one branch runs.
    let ws = file_warnings("(defn f (x) (if (string? x) (string/upper x) (+ x 1)))");
    assert!(!has(&ws, "no argument satisfies"), "{ws:?}");
}

#[test]
fn a_parameter_only_one_use_constrains_is_not_reported() {
    let ws = file_warnings("(defn f (x y) (do (+ x 1) (string/upper y)))");
    assert!(!has(&ws, "no argument satisfies"), "{ws:?}");
}

#[test]
fn an_empty_domain_is_silenced_by_check_allow() {
    let ws =
        file_warnings("(check-allow :type-mismatch (defn b1 (x) (do (+ x 1) (string/upper x))))");
    assert!(!has(&ws, "no argument satisfies"), "{ws:?}");
}

// ---- predicates are inferred as type guards ----

#[test]
fn a_user_predicate_narrows_both_branches() {
    let src = "(defn str-ish? (x) (string? x))\n";
    let ws = file_warnings(&format!("{src}(defn d (x) (if (str-ish? x) (+ x 1) x))"));
    assert!(
        has(&ws, "+: argument 1 expects number, got string"),
        "{ws:?}"
    );
    let ws = file_warnings(&format!(
        "{src}(defn d (x) (if (str-ish? x) x (string/upper x)))"
    ));
    assert!(
        has(&ws, "string/upper: argument 1 expects string"),
        "{ws:?}"
    );
    // …and a correct use stays silent.
    let ws = file_warnings(&format!(
        "{src}(defn d (x) (if (str-ish? x) (string/upper x) 0))"
    ));
    assert!(!has(&ws, "expects"), "{ws:?}");
}

#[test]
fn a_conjunction_predicate_narrows_only_its_true_branch() {
    let src = "(defn pos-int? (x) (and (int? x) (> x 0)))\n";
    let ws = file_warnings(&format!(
        "{src}(defn d (x) (if (pos-int? x) (string/upper x) 0))"
    ));
    assert!(
        has(&ws, "string/upper: argument 1 expects string"),
        "{ws:?}"
    );
    // A false `and` may have failed on its second conjunct: `x` may still be an int.
    let ws = file_warnings(&format!("{src}(defn d (x) (if (pos-int? x) 0 (+ x 1)))"));
    assert!(!has(&ws, "expects"), "{ws:?}");
}

#[test]
fn a_disjunction_predicate_narrows_its_false_branch_to_the_complement() {
    let src = "(defn int-or-str? (x) (or (int? x) (string? x)))\n";
    let ws = file_warnings(&format!(
        "{src}(defn d (x) (if (int-or-str? x) 0 (string/upper x)))"
    ));
    assert!(
        has(&ws, "string/upper: argument 1 expects string"),
        "{ws:?}"
    );
}

#[test]
fn a_predicate_built_from_a_later_predicate_narrows() {
    // `late?`'s return is `bool` from the first round — its tail is an `if` over literals —
    // so the return fixpoint never re-reads it; its guard settles once `str-ish?` has one.
    let ws = file_warnings(
        "(defn d (x) (if (late? x) (+ x 1) 0))\n\
         (defn late? (x) (if (str-ish? x) true false))\n\
         (defn str-ish? (x) (string? x))",
    );
    assert!(
        has(&ws, "+: argument 1 expects number, got string"),
        "{ws:?}"
    );
}

#[test]
fn a_value_returning_function_is_not_a_guard() {
    // Truthy when the key is present — but it answers the value, not a `bool`, so it is
    // no predicate and its truthiness proves nothing about its argument's type.
    let ws = file_warnings("(defn ident (x) x)\n(defn d (x) (if (ident x) 0 (+ x 1)))");
    assert!(!has(&ws, "expects"), "{ws:?}");
}

#[test]
fn an_inferred_guard_does_not_judge_a_refuted_call() {
    // `(refute (pred? bad))` tests the false answer on purpose: only a stated guard (a
    // built-in, a declared `(is T)`) feeds the never-true lint. The lint judges another
    // module's predicates (a same-file one is never judged), so this is `test`'s undeclared
    // `result-skipped?`, whose inferred guard wants a result of at least five fields.
    let ws = file_warnings(
        "(defmodule probe (:use test))\n(defn t () (list (test/result-skipped? (list \"g\" \"t\" nil 0))))",
    );
    assert!(!has(&ws, "can never be true"), "{ws:?}");
    // …while the built-in it is made of still is judged.
    let ws = file_warnings("(defn t () (list (string? (symbol \"x\"))))");
    assert!(has(&ws, "can never be true"), "{ws:?}");
}

#[test]
fn an_inferred_guard_renders_as_a_declarable_is_result() {
    // Two-sided guards are what a `(is T)` declares; the suggestion writes one back.
    let sigs =
        signatures("(defn str-ish? (x) (string? x))\n(defn pos-int? (x) (and (int? x) (> x 0)))");
    assert!(
        sigs.iter()
            .any(|(name, sig, _)| name == "str-ish?" && sig.ends_with("-> (is string)")),
        "{sigs:?}"
    );
    // A one-sided guard has no spelling: the plain result stands in.
    assert!(
        sigs.iter()
            .any(|(name, sig, _)| name == "pos-int?" && sig.ends_with("-> bool")),
        "{sigs:?}"
    );
    // …and the source rendering a "Declare signature" action writes is a real `(is T)`.
    let mut sig = crate::types::Sig::new(vec![Ty::ANY], Ty::of(Tag::Bool));
    sig.guard = Some(Ty::of(Tag::Str));
    sig.guard_else = Some(Ty::of(Tag::Str).negate());
    assert_eq!(sig.to_source().as_deref(), Some("(any -> (is string))"));
    sig.guard_else = None;
    assert_eq!(sig.to_source().as_deref(), Some("(any -> bool)"));
}

// ---- reverse arrows: a demand on a result selects an overload's arms ----

const NEGATE: &str = "(sig negate (and (int -> int) (float -> float)))\n(defn negate (x) x)\n";

#[test]
fn a_demand_on_an_overloaded_result_narrows_its_argument() {
    // `rem` wants an int, and only `negate`'s int arm can answer one: `x` is an int, so a
    // float is reported at the CALL of `nr` — part of `nr`'s own signature.
    let ws = file_warnings(&format!(
        "{NEGATE}(defn nr (x) (math/rem (negate x) 2))\n(defn c () (nr 1.5))"
    ));
    assert!(has(&ws, "nr: argument 1 expects int, got float"), "{ws:?}");
    let ws = file_warnings(&format!(
        "{NEGATE}(defn nr (x) (math/rem (negate x) 2))\n(defn c () (nr 3))"
    ));
    assert!(!has(&ws, "nr: argument"), "{ws:?}");
}

#[test]
fn reverse_arrows_compose_through_nested_overloaded_calls() {
    let sigs = signatures(&format!(
        "{NEGATE}(defn nr2 (x) (math/rem (negate (negate x)) 2))"
    ));
    assert!(
        sigs.iter()
            .any(|(name, sig, _)| name == "nr2" && sig.starts_with("(int) ->")),
        "{sigs:?}"
    );
}

#[test]
fn reverse_arrows_read_inferred_clause_arms() {
    // `cl`'s clauses answer a string only for an int.
    let sigs = signatures(
        "(defn cl ((x) :when (int? x) (str x)) ((x) :when (string? x) (count x)))\n\
         (defn uses-cl (x) (string/upper (cl x)))",
    );
    assert!(
        sigs.iter()
            .any(|(name, sig, _)| name == "uses-cl" && sig.starts_with("(int) ->")),
        "{sigs:?}"
    );
}

#[test]
fn a_demand_every_arm_meets_adds_nothing() {
    // Both arms answer a number: nothing is selected, and `x` keeps the forward reading.
    let sigs = signatures(&format!("{NEGATE}(defn np (x) (+ (negate x) 1))"));
    assert!(
        sigs.iter()
            .any(|(name, sig, _)| name == "np" && !sig.starts_with("(int) ->")),
        "{sigs:?}"
    );
}

// ---- a test bound by `let` narrows wherever its name is tested ----

#[test]
fn a_let_bound_conjunction_narrows_every_conjunct() {
    let ws =
        file_warnings("(defn f (x w) (let (ok (and (string? x) (int? w))) (if ok (+ x w) 0)))");
    assert!(
        has(&ws, "+: argument 1 expects number, got string"),
        "{ws:?}"
    );
    // A false conjunction proves nothing about either conjunct.
    let ws =
        file_warnings("(defn f (x w) (let (ok (and (string? x) (int? w))) (if ok 0 (+ x 1))))");
    assert!(!has(&ws, "expects"), "{ws:?}");
}

#[test]
fn a_negated_bound_test_swaps_its_branches() {
    let ws = file_warnings(
        "(defn f (x w) (let (ok (and (string? x) (int? w)) bad (not ok)) (if bad 0 (string/upper w))))",
    );
    assert!(
        has(&ws, "string/upper: argument 1 expects string, got int"),
        "{ws:?}"
    );
}

#[test]
fn a_negated_conjunction_proves_every_conjunct_when_false() {
    let ws =
        file_warnings("(defn f (x w) (if (not (and (string? x) (int? w))) 0 (string/upper w)))");
    assert!(
        has(&ws, "string/upper: argument 1 expects string, got int"),
        "{ws:?}"
    );
}

#[test]
fn a_bound_disjunction_narrows_its_false_branch() {
    let ws = file_warnings(
        "(defn f (x) (let (ok (or (int? x) (string? x))) (if ok 0 (string/upper x))))",
    );
    assert!(
        has(&ws, "string/upper: argument 1 expects string"),
        "{ws:?}"
    );
}

#[test]
fn a_bound_test_is_a_conjunct_too() {
    let ws = file_warnings(
        "(defn f (x w) (let (ok (and (string? x) (int? w))) (if (and ok (> w 0)) (+ x 1) 0)))",
    );
    assert!(
        has(&ws, "+: argument 1 expects number, got string"),
        "{ws:?}"
    );
}

#[test]
fn a_bound_test_says_nothing_about_a_rebound_variable() {
    // The inner `x` is a new binding: what `ok` proved was about the outer one.
    let ws = file_warnings(
        "(defn f (x w) (let (ok (and (string? x) (int? w))) (let (x 5) (if ok (+ x 1) 0))))",
    );
    assert!(!has(&ws, "expects"), "{ws:?}");
}

// ---- a `match` clause an earlier one covers is unreachable, by type ----

#[test]
fn a_repeated_shape_clause_is_unreachable() {
    let ws = file_warnings("(defn f (x) (match x ((:ok _) 1) ((:error _) 2) ((:ok _) 3) (_ 4)))");
    assert!(has(&ws, "unreachable clause: (:ok _)"), "{ws:?}");
}

#[test]
fn a_narrower_clause_after_a_wider_one_is_unreachable() {
    let ws = file_warnings("(defn f (x) (match x ([:ok _] 1) ([:ok 5] 2) (_ 4)))");
    assert!(has(&ws, "unreachable clause: [:ok 5]"), "{ws:?}");
}

#[test]
fn a_clause_covered_only_by_several_earlier_ones_together_is_unreachable() {
    let ws = file_warnings("(defn f (x) (match x ([:a _] 1) ([:b _] 2) ([(or :a :b) 3] 3) (_ 4)))");
    assert!(has(&ws, "unreachable clause: [(or :a :b) 3]"), "{ws:?}");
}

#[test]
fn a_guarded_clause_does_not_cover_a_later_one() {
    // The guarded clause takes `(:ok _)` only when its guard holds.
    let ws = file_warnings("(defn f (x g) (match x ((:ok _) :when g 1) ((:ok _) 2) (_ 4)))");
    assert!(!has(&ws, "unreachable clause"), "{ws:?}");
}

#[test]
fn a_repeated_binder_is_not_read_as_a_product() {
    // `(y y)` matches only equal pairs: `(5 5)` is one of them, `(5 6)` is not — the
    // pattern is not "any two elements", so it covers nothing it cannot prove.
    let ws = file_warnings("(defn f (x) (match x ((y y) y) ((5 6) 2) (_ 4)))");
    assert!(!has(&ws, "unreachable clause"), "{ws:?}");
}

#[test]
fn distinct_shapes_are_reachable() {
    let ws = file_warnings("(defn f (x) (match x ([:a _] 1) ([:b 1] 2) ([:b 2] 3) (_ 4)))");
    assert!(!has(&ws, "unreachable clause"), "{ws:?}");
}

// ---- refinement through data ----

#[test]
fn a_guard_on_a_field_read_narrows_a_name_bound_from_it() {
    let ws = file_warnings(
        "(defn f (m) (let (foo (get m :foo)) (if (int? (get m :foo)) (string/upper foo) 0)))",
    );
    assert!(
        has(
            &ws,
            "string/upper: argument 1 expects string, got int (foo)"
        ),
        "{ws:?}"
    );
    // …in the else branch too, by the complement.
    let ws = file_warnings(
        "(defn f (m) (let (foo (:foo m)) (if (string? (:foo m)) 0 (string/upper foo))))",
    );
    assert!(
        has(&ws, "string/upper: argument 1 expects string"),
        "{ws:?}"
    );
    let ws =
        file_warnings("(defn f (m) (let (foo (get m :foo)) (if (int? (get m :foo)) (+ foo 1) 0)))");
    assert!(!has(&ws, "expects"), "{ws:?}");
}

#[test]
fn a_key_a_map_does_not_contain_reads_nil() {
    let ws = file_warnings(
        "(defn f (m:(map keyword int)) (if (contains? m :foo) 0 (+ 1 (get m :foo))))",
    );
    assert!(has(&ws, "expects number, got nil"), "{ws:?}");
    let ws = file_warnings(
        "(defn f (m:(map keyword int)) (if (contains? m :foo) (+ 1 (get m :foo)) 0))",
    );
    assert!(!has(&ws, "expects"), "{ws:?}");
    // A value of unknown kind may be a record answering the read through `Lookup`.
    let ws = file_warnings("(defn f (m) (if (contains? m :foo) 0 (+ 1 (get m :foo))))");
    assert!(!has(&ws, "got nil"), "{ws:?}");
}

#[test]
fn an_index_a_step_below_the_count_reads_an_element() {
    // `n` is `(count xs)` and at least 3, so `(- n 2)` is an index into `xs`: an element,
    // not `nil`. This read was reported before the `and` form narrowed at all, and the
    // `if` form reported it always.
    for src in [
        "(defn f (xs:(vector string)) (let (n (count xs)) (if (>= n 3) (string/upper (nth xs (- n 2))) \"\")))",
        "(defn f (xs:(vector string)) (let (n (count xs)) (and (>= n 3) (string/upper (nth xs (- n 2))))))",
        "(defn f (xs:(vector string)) (if (>= (count xs) 1) (string/upper (nth xs (- (count xs) 1))) \"\"))",
    ] {
        let ws = file_warnings_mode(src, true);
        assert!(!has(&ws, "string/upper"), "{src}: {ws:?}");
    }
    // …and the element type is what the read answers: a string handed to `+` is reported.
    let ws = file_warnings(
        "(defn f (xs:(vector string)) (let (n (count xs)) (if (>= n 3) (+ 1 (nth xs (- n 2))) 0)))",
    );
    assert!(has(&ws, "got string"), "{ws:?}");
    // Without the lower bound the index may be negative: `nil` stays possible.
    let ws = file_warnings_mode(
        "(defn f (xs:(vector string)) (let (n (count xs)) (string/upper (nth xs (- n 2)))))",
        true,
    );
    assert!(has(&ws, "string/upper"), "{ws:?}");
}

#[test]
fn an_offset_bound_by_let_or_written_another_way_reads_an_element() {
    // The count-minus-k bound is a fact about the index's BASE, so it holds however the
    // offset is spelled and wherever it is bound: a `let` of `(dec size)` or `(- n 2)`, `(+ size -1)`,
    // and an offset of an offset. (The guard comes first: an index bound before it keeps the
    // interval it was bound with — relating the two is not this.)
    for src in [
        "(defn read-one (xs:(vector string)) (let (size (count xs)) (if (>= size 1) (let (last (dec size)) (string/upper (nth xs last))) \"\")))",
        "(defn read-one (xs:(vector string)) (let (size (count xs)) (if (>= size 1) (string/upper (nth xs (+ size -1))) \"\")))",
        "(defn read-one (xs:(vector string)) (let (size (count xs)) (if (>= size 3) (let (before (- size 2) after (inc before)) (string/upper (nth xs after))) \"\")))",
        "(defn read-one (xs:(vector string)) (if (>= (count xs) 1) (let (last (dec (count xs))) (string/upper (nth xs last))) \"\"))",
    ] {
        let ws = file_warnings_mode(src, true);
        assert!(!has(&ws, "string/upper"), "{src}: {ws:?}");
    }
    // An offset that reaches the count itself is not an index: `nil` stays possible.
    for src in [
        "(defn read-one (xs:(vector string)) (let (size (count xs)) (if (>= size 3) (let (before (- size 1) after (inc before)) (string/upper (nth xs after))) \"\")))",
        "(defn read-one (xs:(vector string)) (let (size (count xs)) (if (>= size 3) (string/upper (nth xs size)) \"\")))",
    ] {
        let ws = file_warnings_mode(src, true);
        assert!(has(&ws, "string/upper"), "{src}: {ws:?}");
    }
}

#[test]
fn a_scope_dead_on_arrival_does_not_close_an_ors_paths() {
    // `covered` is `count-of`'s result, `never` while its return is still at the fixpoint's
    // first round; the `or` used as a test then saw both of its paths dead and claimed
    // nothing, so `size` reached `inclusion` unnarrowed and `path`'s caller-derived `end`
    // widened — a strict report on `n` that mur-forge's log.blsp raised (2026-11-16).
    let src = "(defn count-of (value) (if (int? value) (+ value 1) 0))\n\
         (defn- lpb (width:int) (if (<= width 2) 1 (* 2 (lpb (math/quot width 2)))))\n\
         (defn- path (index start end)\n\
           (let (width (- end start))\n\
             (if (= width 1) [] (let (half (lpb width)) (if (< index half) (conj (path index start (+ start half)) half) (conj (path (- index half) (+ start half) end) half))))))\n\
         (defn inclusion (index size) (path index 0 size))\n\
         (defn- locked (action) (action 1))\n\
         (defn proof (index size)\n\
           (locked (fn (tx) (let (covered (count-of tx))\n\
             (cond (or (not (int? size)) (< size 1)) [:error \"size\"] else [:ok (inclusion index size)])))))";
    let ws = file_warnings_mode(src, true);
    assert!(!has(&ws, "lpb: argument 1"), "{ws:?}");
}

// ---- review findings (each was a false positive on a valid program) ----

#[test]
fn an_or_over_a_guard_proving_only_when_false_keeps_its_else_branch() {
    // `p?` proves `¬string` when false and nothing when true; the else branch of the `or`
    // is reachable (`(f 5.5)` is nil), so nothing may call the outer test always true…
    let src = "(defn opaque (x) (> (count (str x)) 3))\n\
               (defn p? (x) (or (string? x) (opaque x)))\n\
               (defn f (y) (if (or (p? y) (int? y)) :yes nil))\n";
    let ws = file_warnings(&format!("{src}(defn h (y) (if (f y) 1 2))"));
    assert!(!has(&ws, "always"), "{ws:?}");
    // …and that branch is checked, under what both falsy disjuncts prove.
    let ws = file_warnings(&format!(
        "{src}(defn k (y) (if (or (p? y) (int? y)) 1 (string/upper y)))"
    ));
    assert!(
        has(&ws, "string/upper: argument 1 expects string"),
        "{ws:?}"
    );
}

#[test]
fn an_or_pattern_past_the_widening_cap_covers_only_its_alternatives() {
    // Five alternatives: a widened union would hull `[:d 4]` and `[:e 5]` into
    // `[(:d|:e) (4|5)]` and claim `[:d 5]`.
    let ws = file_warnings(
        "(defn f (v) (match v ((or [:a 1] [:b 2] [:c 3] [:d 4] [:e 5]) 1) ([:d 5] 2) (_ 3)))",
    );
    assert!(!has(&ws, "unreachable clause"), "{ws:?}");
}

#[test]
fn an_inferred_guard_does_not_make_a_refuted_test_dead() {
    // `refute` binds its test to a name first: the alias carries where the guard came from.
    let ws = file_warnings(
        "(defmodule probe (:use test))\n\
         (defn str-ish? (x) (string? x))\n\
         (defn t () (let (bad 5) (refute (str-ish? bad))))",
    );
    assert!(!has(&ws, "unreachable clause"), "{ws:?}");
    // A built-in still judges.
    let ws = file_warnings(
        "(defmodule probe (:use test))\n(defn t () (let (bad 5) (refute (string? bad))))",
    );
    assert!(has(&ws, "unreachable clause"), "{ws:?}");
}

#[test]
fn a_hand_written_chain_binder_shadows_the_names_it_binds() {
    // The later operand reads the chain's own `v` / `ok`, not the scope the branches see.
    for src in [
        "(defn f (v m) (if (let (v (get m :v)) (if v (string? v) v)) (+ v 1) 0))",
        "(defn f (v m) (let (ok (let (v (get m :v)) (if v (string? v) v))) (if ok (+ v 1) 0)))",
        "(defn f (x y) (let (ok (int? y)) (if (let (ok (string? x)) (if ok ok ok)) (string/upper y) \"\")))",
    ] {
        let ws = file_warnings(src);
        assert!(!has(&ws, "expects"), "{src}: {ws:?}");
    }
}

#[test]
fn an_empty_domain_counts_ability_impls_another_module_may_add() {
    // `+` takes any record a `num/*` impl covers, and `count` any `Counted` one: an impl in
    // a downstream module can make `total` work, so it is not reported here.
    let ws = file_warnings("(defn total (x) (count x) (+ x 1))");
    assert!(!has(&ws, "no argument satisfies"), "{ws:?}");
    // A conflict no record can resolve still is.
    let ws = file_warnings("(defn f (x) (do (string/upper x) (+ x 1)))");
    assert!(has(&ws, "no argument satisfies"), "{ws:?}");
}

#[test]
fn a_declared_guard_over_a_body_that_raises_first_is_not_contradicted() {
    // The tail admits floats, but the first form raised for them: `(is int)` is honest.
    let ws = file_warnings(
        "(sig f (any -> (is int)))\n(defn f (x) (when (not (int? x)) (error \"no\")) (number? x))",
    );
    assert!(!has(&ws, "declared (is"), "{ws:?}");
}

// A conjunct's PATH guard narrows an `and`'s then-branch as surely as a local's guard does.
// Only the variable guards were layered, so `(and (int? (get loop :began)) (int? now))` left
// the field a `number` in `(- now (get loop :began))` while the same guard alone narrowed it
// (zubr's pane, under `--strict`).
#[test]
fn an_and_narrows_a_field_path_in_its_then_branch() {
    for test in [
        "(and (int? (get loop :began)) (int? now))",
        "(and (int? now) (int? (get loop :began)))",
        "(and (int? (:began loop)) (int? now))",
    ] {
        let src = format!(
            "(defmodule t)\n\
             (defn elapsed (now loop)\n  \
               (if {test} (math/quot (- now (get loop :began)) 1000) nil))"
        );
        let ws = file_warnings_mode(&src, true);
        assert!(ws.is_empty(), "{test}: {ws:?}");
    }
}

// A bare truthiness test of a field the checker cannot type leaves it unknown. Narrowed to
// "anything but nil or false" — a POSITIVE type wider than every use accepts — `error-message`
// (`(and (map? e) (get e :message)) (get e :message)`) returned that type, so `(first report)`
// on a `try` that caught into it was a plain finding (bedit's `about.blsp`), and so was a
// direct `(if (get e :k) (get e :k) …)` helper's result.
#[test]
fn a_truthiness_test_of_an_unknown_field_leaves_it_unknown() {
    for source in [
        "(defmodule t)\n\
         (defn- facts (window) [window 2 3])\n\
         (defn show (window)\n  \
           (let (report (try (facts window) (catch e (error-message e))))\n    \
             (if (string? report) report (first report))))",
        "(defmodule t)\n\
         (defn msg (e) (if (get e :message) (get e :message) \"none\"))\n\
         (defn show (e) (let (m (msg e)) (if (string? m) m (first m))))",
    ] {
        let ws = file_warnings_mode(source, false);
        assert!(ws.is_empty(), "{source}: {ws:?}");
    }
    // A field the checker CAN type still loses its nil under the same test.
    let known = file_warnings_mode(
        "(defmodule t)\n\
         (defn f (r: (record :n (or nil int))) (if (get r :n) (+ (get r :n) 1) 0))",
        true,
    );
    assert!(known.is_empty(), "{known:?}");
}
