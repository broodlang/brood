//! Step 4: scope tracking and guard narrowing, arity and unbound-symbol diagnostics, operand-slot unbound symbols, `%eq` as a guard, let-alias propagation.

use super::*;

// ------------- Step 4: scope tracking + guard narrowing --------------

#[test]
fn let_binding_propagates_its_rhs_type() {
    // The RHS is a literal int — `(first x)` should flag, because x : int
    // shadows "unknown" in the body. (This is the basic let-tracking.)
    let w = warnings("(let (x 1) (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 1")),
        "expected a `first x` warning where x : 1 (int singleton), got {:?}",
        w
    );
}

#[test]
fn let_binding_from_nested_call_propagates() {
    // RHS is a known primitive whose return type is int. So `x : int`,
    // and `(first x)` flags.
    let w = warnings("(let (x (string/length \"hi\")) (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected a `first x` warning where x : int, got {:?}",
        w
    );
}

#[test]
fn let_binding_of_unknown_rhs_stays_silent() {
    // RHS is a variable (unknown), so x stays unknown — `(first x)` must
    // not warn. (No false positives from let-tracking.)
    let w = warnings("(let (x foo) (first x))");
    assert!(w.is_empty(), "got {:?}", w);
}

#[test]
fn inner_let_shadows_outer_binding() {
    // The outer x : int; the inner x : string. `(first x)` in the body
    // refers to the inner, which is a string — and `first` accepts list /
    // vector, disjoint from string. So a warning is still expected, but
    // the *narrowing message* must be "string", not "int". This is the
    // shadowing-correctness check (outer narrowing must not leak in).
    let w = warnings("(let (x 1) (let (x \"hi\") (first x)))");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("got \"hi\"")),
        "expected the inner string to be the source, got {:?}",
        w
    );
    assert!(
        // Outer `x` is the literal `1` → singleton `{1}`; if it leaked the
        // message would say "got 1" (B0 — was "got int").
        !w.iter().any(|s| s.contains("got 1")),
        "outer int must not leak through shadowing: {:?}",
        w
    );
}

#[test]
fn shadowing_with_unknown_rhs_clears_prior_narrowing() {
    // Outer x : int; inner x : <unknown var>. Inside the inner let, x is
    // unknown — `(first x)` must NOT warn (the outer narrowing must not
    // leak through the shadow).
    let w = warnings("(let (x 1) (let (x foo) (first x)))");
    assert!(w.is_empty(), "shadow must clear the prior type: {:?}", w);
}

#[test]
fn vector_let_bindings_are_recognised() {
    // The bindings container is a LIST (ADR-010) — the vector shape is a compile
    // error now, so the checker only ever sees this spelling.
    let w = warnings("(let (x 1) (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 1")),
        "vector-form let bindings must populate the ctx: {:?}",
        w
    );
}

#[test]
fn guard_narrowing_lets_a_then_branch_flag_a_misuse() {
    // In the then-branch of `(if (int? x) …)`, x : int — `(first x)` flags.
    let w = warnings("(if (int? x) (first x) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected guard narrowing to flag (first x) when x : int, got {:?}",
        w
    );
}

#[test]
fn guard_narrowing_does_not_leak_into_the_else_branch() {
    // The else-branch narrows x to `not int`, which overlaps list / vector;
    // so `(first x)` must NOT warn there.
    let w = warnings("(if (int? x) nil (first x))");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "else branch must not have x narrowed to int: {:?}",
        w
    );
}

#[test]
fn negated_guard_flips_the_narrowing() {
    // (if (not (int? x)) …) — the then-branch narrows x to `not int`, the
    // else-branch to int.
    let w = warnings("(if (not (int? x)) nil (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "the else of a negated guard must narrow to the inner type: {:?}",
        w
    );
}

#[test]
fn guards_for_number_and_list_unions_narrow_to_the_union() {
    // (if (number? x) (first x) …) — x : number = int|float in the then,
    // which is disjoint from list/vector, so `(first x)` flags.
    let w = warnings("(if (number? x) (first x) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("number")),
        "number? must narrow to int|float: {:?}",
        w
    );
    // The list? guard should *not* warn in the then (list overlaps first's
    // expected type).
    let w = warnings("(if (list? x) (first x) nil)");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "list? must not produce a false positive on (first x): {:?}",
        w
    );
}

#[test]
fn failure_narrowing_clears_the_failure_from_the_else_branch() {
    // ADR-310 appended `Failure` as a tag but not to the predicate table, so
    // `failure?` narrowed nothing: the idiomatic default — the very shape
    // `failure?`'s own docstring teaches — left `number | failure` flowing into
    // arithmetic, and std went red on 14 strict warnings it could not silence.
    let w = warnings("(let (n (string/->number s)) (if (failure? n) 0 (inc n)))");
    assert!(
        !w.iter().any(|s| s.contains("inc")),
        "the else of (failure? n) must narrow n to number: {w:?}"
    );
    // And the then-branch really does hold a failure — a failure is not a list.
    let w = warnings("(if (failure? x) (first x) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("failure")),
        "the then-branch of (failure? x) must narrow x to failure: {w:?}"
    );
}

#[test]
fn and_narrows_the_then_branch_on_every_conjunct() {
    // A truthy `and` proves ALL conjuncts, so the second (and third) conjunct's
    // narrowing must reach the then-branch, not just the first (ADR-011 gap close).
    let w = warnings_expanded("(if (and (int? a) (string? b)) (+ 1 b) 0)");
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("string")),
        "the 2nd `and` conjunct should narrow b to string in the then-branch: {w:?}"
    );
    let w3 = warnings_expanded("(if (and (int? a) (int? b) (string? c)) (+ c 1) 0)");
    assert!(
        w3.iter().any(|s| s.contains("+") && s.contains("string")),
        "the 3rd `and` conjunct should narrow c to string: {w3:?}"
    );
}

#[test]
fn and_falsy_does_not_narrow_the_else_branch() {
    // A falsy `and` may have failed on any conjunct, so it proves NOTHING — the
    // else-branch must not be narrowed (would be a false positive).
    let w = warnings_expanded("(if (and (int? a) (string? b)) 0 (+ 1 b))");
    assert!(
        !w.iter().any(|s| s.contains("+")),
        "a falsy `and` must not narrow the else-branch: {w:?}"
    );
}

#[test]
fn or_same_var_narrows_both_branches() {
    // Every disjunct a biconditional guard over the same var: the then-branch is the
    // union (a truthy `or` ⇒ some disjunct holds), the else-branch its complement (a
    // falsy `or` ⇒ none hold). So `(string/length c)` in the else flags — c is not string.
    let w = warnings_expanded("(if (or (nil? c) (string? c)) 0 (string/length c))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "the else of an all-same-var `or` should narrow c to ¬(nil|string): {w:?}"
    );
    // But a valid use in either branch stays silent — `str` accepts anything, and the
    // then-branch is the union (nil|string), which overlaps everything `str` wants.
    let ok = warnings_expanded("(if (or (nil? c) (string? c)) (str c) (str c))");
    assert!(
        ok.is_empty(),
        "a valid `or`-guarded use must stay silent: {ok:?}"
    );
}

#[test]
fn or_over_different_vars_does_not_narrow() {
    // Disjuncts over *different* variables give no single-variable narrowing — the
    // else-branch must not flag a use of either (would be a false positive).
    let w = warnings_expanded("(if (or (nil? a) (string? b)) 0 (string/length a))");
    assert!(
        !w.iter().any(|s| s.contains("string/length")),
        "an `or` over different vars must not narrow: {w:?}"
    );
}

#[test]
fn non_guard_tests_dont_narrow() {
    // The test isn't a recognised type predicate, so x stays unknown in
    // both branches — `(first x)` must not warn.
    let w = warnings("(if (math/zero? x) (first x) (first x))");
    assert!(w.is_empty(), "non-tag-guard test must not narrow: {:?}", w);
}

#[test]
fn nested_guards_compose_their_narrowings() {
    // (if (number? x) (if (int? x) … (first x)) …) — in the inner else,
    // x is narrowed to `number ∩ ¬int` = float, which is still disjoint
    // from list/vector, so `(first x)` flags.
    let w = warnings("(if (number? x) (if (int? x) nil (first x)) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("float")),
        "nested guards must compose to float (= number ∩ ¬int): {:?}",
        w
    );
}

#[test]
fn let_bound_guard_narrows_when_used_as_an_if_test() {
    // The user-written shape `(let (cond (int? x)) (if cond …))` — Brood is
    // immutable, so `cond` faithfully reflects `(int? x)` until the let
    // ends. The guard-alias table maps `cond → (x, int)`, and the inner
    // `if cond` narrows x to int in the then-branch.
    let w = warnings("(let (cond (int? x)) (if cond (first x) nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected let-bound guard to flag (first x) in the then: {:?}",
        w
    );
}

#[test]
fn let_bound_guard_narrows_in_the_else_branch_too() {
    // Else-branch sees x as `not int`, which overlaps list / vector, so
    // no warning — same as the direct-test case.
    let w = warnings("(let (cond (int? x)) (if cond nil (first x)))");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "the else of a let-bound guard must narrow to ¬int, not int: {:?}",
        w
    );
}

#[test]
fn let_bound_guard_can_be_negated_in_the_if() {
    // `(if (not cond) …)` flips the narrowing — same as `(not (int? x))`.
    let w = warnings("(let (cond (int? x)) (if (not cond) nil (first x)))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected negation to flip the let-bound guard: {:?}",
        w
    );
}

#[test]
fn rebinding_the_guard_name_clears_the_alias() {
    // After `(let (cond <unknown>) …)` shadowing, `cond` no longer aliases
    // the int-guard, so `(if cond …)` must not narrow x.
    let w = warnings("(let (cond (int? x)) (let (cond foo) (if cond (first x) nil)))");
    assert!(w.is_empty(), "shadowing must drop the guard alias: {:?}", w);
}

#[test]
fn rebinding_to_a_non_guard_value_clears_the_alias() {
    // Same as above but with an int literal rather than an unknown var.
    let w = warnings("(let (cond (int? x)) (let (cond 1) (if cond (first x) nil)))");
    assert!(
        w.is_empty(),
        "shadowing with a non-guard value must drop the alias: {:?}",
        w
    );
}

#[test]
fn self_aliased_guard_is_not_recorded() {
    // `(let (x (int? x)) …)` shadows the outer x with a bool; the inner
    // body's `x` is the bool, not the original — narrowing the original
    // would be unsound (it's no longer reachable), so we must not record
    // the guard. (No assertion about a warning either way — the point is
    // we don't crash and don't introduce a stale alias.)
    let w = warnings("(let (x (int? x)) (if x x nil))");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "self-aliased guards must not propagate to inner uses: {:?}",
        w
    );
}

#[test]
fn let_inside_a_then_branch_can_shadow_a_narrowing() {
    // Outer narrowing: x : int. Inner shadow: x : string. The body now
    // sees x as string, so the narrowing message names string.
    let w = warnings("(if (int? x) (let (x \"hi\") (first x)) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("got \"hi\"")),
        "shadow must override the guard narrowing: {:?}",
        w
    );
    assert!(
        !w.iter().any(|s| s.contains("got int")),
        "the int narrowing must not leak through the shadow: {:?}",
        w
    );
}

// ---------------- Step 4: arity + unbound-symbol diagnostics ----------------

#[test]
fn flags_too_few_arguments() {
    // `first` expects exactly 1; 0 is wrong.
    assert!(warnings("(first)")
        .iter()
        .any(|w| w.contains("first") && w.contains("expected 1") && w.contains("got 0")));
    // `string-length` expects exactly 1.
    assert!(warnings("(string/length)")
        .iter()
        .any(|w| w.contains("string/length") && w.contains("expected 1")));
}

#[test]
fn flags_too_many_arguments() {
    // `inc` is `exact(1)`; calling with 2 is wrong. This used to use `rem`, which moved to
    // `math` on 2026-08-27 — and `warnings()` builds a bare `Interp::new()` with no module
    // loaded, so a `math/…` name has no bound function for the ARITY check to read (the
    // curated sig supplies types only). A still-bare function keeps the test about what it
    // is about.
    assert!(warnings("(inc 1 2)")
        .iter()
        .any(|w| w.contains("inc") && w.contains("expected 1") && w.contains("got 2")));
}

#[test]
fn arity_message_handles_range_and_variadic() {
    // `%map-get` is `range(2, 3)` → "expected 2 to 3".
    assert!(warnings("(%map-get {})")
        .iter()
        .any(|w| w.contains("%map-get") && w.contains("2 to 3")));
    // `apply` is `at_least(2)` → "expected at least 2 arguments"; 1 is too few.
    assert!(warnings("(apply f)")
        .iter()
        .any(|w| w.contains("apply") && w.contains("at least 2 arguments")));
}

#[test]
fn arity_pass_is_silent_for_correct_calls() {
    assert!(warnings("(first [1 2])")
        .iter()
        .all(|w| !w.contains("number of arguments")));
    assert!(warnings("(math/rem 7 3)")
        .iter()
        .all(|w| !w.contains("number of arguments")));
    // Variadic: any count is fine.
    for n in 0..=5 {
        let args = (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        let w = warnings(&format!("(+ {})", args));
        assert!(
            w.iter().all(|s| !s.contains("number of arguments")),
            "(+ {}…) should not warn arity: {:?}",
            n,
            w
        );
    }
}

#[test]
fn flags_unbound_call_heads() {
    assert!(warnings("(frobnicate 1)")
        .iter()
        .any(|w| w.contains("unbound symbol: frobnicate")));
    assert!(warnings("(typo-name :hi)")
        .iter()
        .any(|w| w.contains("unbound symbol: typo-name")));
}

// ---- Operand / value-slot unbound symbols (whole-file mode only) --------

#[test]
fn flags_unbound_operand_of_a_known_call() {
    // `+` evaluates its args, so a bare unresolvable operand is unbound.
    let w = file_warnings("(defn f (x) (+ x typo))");
    assert!(
        w.iter().any(|m| m.contains("unbound symbol: typo")),
        "operand typo should be flagged: {:?}",
        w
    );
    // Through a primitive too (cons), nested under a body.
    let w = file_warnings("(defn g () (cons 1 nope))");
    assert!(
        w.iter().any(|m| m.contains("unbound symbol: nope")),
        "{:?}",
        w
    );
}

#[test]
fn flags_unbound_value_in_def_let_if_slots() {
    assert!(file_warnings("(def y zilch)")
        .iter()
        .any(|m| m.contains("unbound symbol: zilch")));
    assert!(file_warnings("(defn f () (let (a absent) a))")
        .iter()
        .any(|m| m.contains("unbound symbol: absent")));
    assert!(file_warnings("(defn f () (if missing 1 2))")
        .iter()
        .any(|m| m.contains("unbound symbol: missing")));
}

#[test]
fn operand_check_respects_scope_and_forward_refs() {
    // A forward reference to a later top-level def — file-global, not unbound.
    assert!(file_warnings("(defn a () (cons 1 (b)))\n(defn b () 2)")
        .iter()
        .all(|m| !m.contains("unbound")));
    // A param / let-bound name used as an operand — in scope, not unbound.
    assert!(file_warnings("(defn f (x) (+ x 1))")
        .iter()
        .all(|m| !m.contains("unbound")));
    assert!(file_warnings("(defn f () (let (y 1) (+ y 2)))")
        .iter()
        .all(|m| !m.contains("unbound")));
    // A prelude name as an operand resolves through the heap globals.
    assert!(file_warnings("(defn f () (map (list 1 2) inc))")
        .iter()
        .all(|m| !m.contains("unbound")));
}

#[test]
fn operand_check_is_off_for_bare_fragments() {
    // The single-form path (REPL / `(check 'form)`) stays lenient: a free
    // operand variable is ambiguous, not provably unbound — only call *heads*
    // are flagged there. (Guards the no-false-positives rule for fragments.)
    assert!(warnings("(first xs)")
        .iter()
        .all(|m| !m.contains("unbound")));
    assert!(warnings("(+ 1 foo)").iter().all(|m| !m.contains("unbound")));
    assert!(warnings("(let (x bar) (first x))")
        .iter()
        .all(|m| !m.contains("unbound")));
}

#[test]
fn flags_zero_arg_fn_passed_bare_to_an_output_sink() {
    // The `(print ansi-clear)`-for-`(print (ansi-clear))` slip: a bare
    // zero-arity global handed to print/println/str/format stringifies the
    // function (#<fn …>), never its result — silent today.
    for sink in &["print", "println", "str", "format"] {
        let w = check_with_defs(&["(defn home () \"\\e[H\")"], &format!("({} home)", sink));
        assert!(
            w.iter()
                .any(|m| m.contains("home: function used as a value")
                    && m.contains("did you mean (home)")),
            "{} should flag a bare zero-arg fn: {:?}",
            sink,
            w
        );
    }
}

#[test]
fn function_as_value_lint_is_quiet_on_the_correct_and_legitimate_shapes() {
    // Called correctly — no warning.
    assert!(
        check_with_defs(&["(defn home () \"\\e[H\")"], "(print (home))")
            .iter()
            .all(|m| !m.contains("function used as a value"))
    );
    // A fn that *takes* arguments is a plausible intentional callback value.
    assert!(check_with_defs(&["(defn f (x) x)"], "(print f)")
        .iter()
        .all(|m| !m.contains("function used as a value")));
    // A same-named *local* (not the global zero-arg fn) is left alone.
    assert!(
        check_with_defs(&["(defn home () 1)"], "(let (home 42) (print home))")
            .iter()
            .all(|m| !m.contains("function used as a value"))
    );
    // A plain value is fine.
    assert!(warnings("(print 42)")
        .iter()
        .all(|m| !m.contains("function used as a value")));
    // The lint is sink-scoped: passing a bare zero-arg fn elsewhere (a real
    // higher-order use) is not flagged.
    assert!(check_with_defs(&["(defn home () 1)"], "(map [1 2] home)")
        .iter()
        .all(|m| !m.contains("function used as a value")));
}

#[test]
fn unbound_is_silent_for_in_scope_names() {
    // fn params don't look unbound when used as call heads or
    // referenced in the body.
    assert!(warnings("(fn (f) (f 1 2))")
        .iter()
        .all(|w| !w.contains("unbound")));
    // let bindings: same.
    assert!(warnings("(let (g (fn (x) x)) (g 1))")
        .iter()
        .all(|w| !w.contains("unbound")));
    // Syntactic keywords aren't bound but are never "unbound".
    for src in &["(do 1 2 3)", "(when true 1)", "(cond)", "(and)", "(or)"] {
        assert!(
            warnings(src).iter().all(|w| !w.contains("unbound")),
            "syntactic keyword must not be flagged unbound: {} → {:?}",
            src,
            warnings(src)
        );
    }
}

#[test]
fn unbound_is_silent_for_prelude_names() {
    // The prelude is loaded in our test heap (via Interp::new()), so
    // stdlib names resolve. `inc`, `list`, `int?`, `even?`, … are all fine.
    for src in &[
        "(inc 1)",
        "(list 1 2 3)",
        "(int? 5)",
        "(math/zero? 0)",
        "(map [1 2 3] (fn (x) x))",
    ] {
        assert!(
            warnings(src).iter().all(|w| !w.contains("unbound")),
            "prelude name must not be flagged unbound: {} → {:?}",
            src,
            warnings(src)
        );
    }
}

#[test]
fn unbound_roots_a_bare_name_against_the_current_compile_ns() {
    // The checker mirrors eval's `compile_ns` rooting (ADR-070): after `%in-ns`, a bare
    // name that resolves to `<ns>/name` is not flagged unbound. This is the REPL case —
    // `nest repl` enters the project's `:main` namespace so a bare project fn resolves at
    // the prompt without the advisory checker crying "unbound".
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(%in-ns 'ns1) (defn foo () 1)")
        .expect("defines ns1/foo");
    // `eval_str` resets `compile_ns` on return; re-establish it the way the REPL loop
    // process holds it across interactive `reflect/eval-string`s (which don't reset).
    interp
        .heap
        .set_compile_ns(Some(crate::core::value::intern("ns1")));

    // A bare `foo` roots to the bound `ns1/foo` → no unbound warning.
    let bound = reader::read_one(&mut interp.heap, "(foo)").expect("parse");
    let w_bound = check_form(&interp.heap, bound);
    assert!(
        w_bound.iter().all(|m| !m.contains("unbound")),
        "a bare `foo` should root to ns1/foo (bound) and not warn, got {w_bound:?}"
    );

    // A genuinely unbound bare name is still flagged (rooting only ever *finds*, never masks).
    let unbound = reader::read_one(&mut interp.heap, "(nope-xyz)").expect("parse");
    let w_unbound = check_form(&interp.heap, unbound);
    assert!(
        w_unbound
            .iter()
            .any(|m| m.contains("unbound") && m.contains("nope-xyz")),
        "a genuinely unbound name must still be flagged, got {w_unbound:?}"
    );
}

#[test]
fn file_globals_make_later_forms_see_earlier_defs() {
    // `check_file` accumulates top-level def names. Without that,
    // `(my-fn 1)` in form 2 would be flagged unbound — `my-fn` isn't in
    // the heap (no eval), only in the file.
    let interp = crate::Interp::new();
    let src = "(defn my-fn (x) (+ x 1))\n(my-fn 1)";
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    let out = check_file(&mut heap, &forms);
    let msgs: Vec<_> = out.into_iter().map(|(_, m)| m).collect();
    assert!(
        msgs.iter().all(|m| !m.contains("unbound symbol: my-fn")),
        "file-local defns must shield later calls: {:?}",
        msgs
    );
}

#[test]
fn fn_params_with_rest_and_optional_dont_leak() {
    // The marker symbols `&`/`&optional` themselves are *not* binders;
    // the names that follow them are.
    assert!(warnings("(fn (x & ys) (cons x ys))")
        .iter()
        .all(|w| !w.contains("unbound")));
    assert!(warnings("(fn (x &optional (y 0)) (+ x y))")
        .iter()
        .all(|w| !w.contains("unbound")));
}

#[test]
fn defn_body_sees_its_params_in_scope() {
    // A user defn whose body references its params must not flag them as
    // unbound. (The `defn` macro hasn't been expanded — the CLI checks
    // un-expanded forms — so this tests the un-expanded surface path.)
    assert!(warnings("(defn my-fn (x y) (+ x y))")
        .iter()
        .all(|w| !w.contains("unbound")));
}

#[test]
fn arity_check_works_for_user_defns_in_a_real_interp() {
    // Once a defn is evaluated, its arity is derivable from its Closure.
    // `inc` (prelude) is `(defn inc (n) …)` → exact(1).
    let w = check_with_defs(&[], "(inc 1 2)");
    assert!(
        w.iter()
            .any(|s| s.contains("inc") && s.contains("expected 1")),
        "user defn arity should be enforced: {:?}",
        w
    );
}

// ---- Step 4 final pieces: %eq-as-guard + let-alias propagation --------
//
// `match` lowers `(match x (5 body) …)` to
// `(let (m__N x) (if (%eq m__N 5) (do body) …))`. To flag a misuse on
// `x` in `body` (where the literal pattern asserts x's type), the checker
// needs two pieces: (1) recognise `(%eq sym lit)` as a guard asserting
// `sym : type-of(lit)`; (2) when a `let` binds a name to another symbol,
// propagate narrowings between the two via the alias chain.

#[test]
fn match_literal_pattern_narrows_the_scrutinee() {
    // `(match x (5 (first x)))` — the literal-int pattern asserts x : int;
    // `(first x)` in the body must then flag. Goes through macroexpansion
    // because `match` is a `defmacro` whose pattern compiler lowers to
    // `let`+`if`+`%eq`; the checker's narrowing rides the lowered shape.
    let w = warnings_expanded("(match x (5 (first x)) (_ nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "match int-literal pattern should narrow x: {:?}",
        w
    );
}

#[test]
fn match_keyword_pattern_narrows_the_scrutinee() {
    // Mirror of the int case for a keyword literal. The scrutinee narrows to
    // the literal singleton `:foo`, so the diagnostic names that exact value.
    let w = warnings_expanded("(match x (:foo (first x)) (_ nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains(":foo")),
        "match keyword-literal pattern should narrow x: {:?}",
        w
    );
}

#[test]
fn eq_against_a_literal_is_a_guard() {
    // The mechanism that powers match: `(%eq m 5)` in a test position
    // narrows `m` to `:int` in the then-branch. (Symmetric — both
    // `(%eq m 5)` and `(%eq 5 m)` should narrow.)
    let w = warnings("(if (%eq m 5) (first m) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "%eq with sym + literal should narrow: {:?}",
        w
    );
    let w = warnings("(if (%eq 5 m) (first m) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "%eq with literal + sym (reversed) should narrow: {:?}",
        w
    );
}

#[test]
fn eq_between_two_variables_is_not_a_guard() {
    // Equality between two unknowns asserts nothing about either's type.
    // No false positive must fire on the body.
    let w = warnings("(if (%eq a b) (first a) nil)");
    assert!(
        w.iter().all(|s| !s.contains("first")),
        "%eq between two vars should not narrow: {:?}",
        w
    );
}

#[test]
fn eq_guard_does_not_narrow_the_else_branch() {
    // `(= m "x")` being *false* does NOT prove `m` isn't a string — it could
    // be another string. So the else-branch must not narrow `m` to `¬string`
    // and flag a valid `(string/length m)`. (Same then-only soundness as the
    // `and` guard.)
    let w = warnings(r#"(if (%eq m "x") :yes (string/length m))"#);
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "the else-branch of an `=`/`%eq` guard must not be narrowed: {w:?}"
    );
    // The then-branch must still narrow (sanity): `(= m 5)` true ⇒ m : int.
    let w = warnings("(if (%eq m 5) (first m) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "the then-branch must still narrow m to int: {w:?}"
    );
}

#[test]
fn let_alias_propagates_narrowing_in_both_directions() {
    // The match pattern compiler's exact shape: alias `m` to `x`, then
    // narrow `m` via a guard. The narrowing must flow back onto `x` so a
    // body that uses `x` (not `m`) still sees the asserted type.
    let w = warnings("(let (m x) (if (int? m) (first x) nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "let-alias should propagate narrowing from m to x: {:?}",
        w
    );
    // And the symmetric direction: narrow x, alias-narrows m.
    let w = warnings("(let (m x) (if (int? x) (first m) nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "let-alias should propagate narrowing from x to m: {:?}",
        w
    );
}

#[test]
fn shadowing_clears_an_alias() {
    // An inner let that rebinds an aliased name to something else breaks
    // the chain — the new binding is the new name's type, no alias.
    // `(let (m x) (let (m 5) (first m)))` flags the inner `(first m)`
    // because `m` is now int, but that's via the literal-type binding,
    // not the broken alias.
    let w = warnings("(let (m x) (let (m 5) (first m)))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "shadowed let should still warn on the inner int: {:?}",
        w
    );
    // The outer `x` must not be narrowed by the inner shadowing.
    let w = warnings("(let (m x) (let (m 5) (io/puts x)))");
    assert!(
        w.iter().all(|s| !s.contains("first")),
        "shadowing must not leak narrowing back to the original: {:?}",
        w
    );
}
