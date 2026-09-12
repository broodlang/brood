//! Effective signatures for a buffer — the LSP inlay-hint source — and callback results under the combinators.

use super::*;

#[test]
fn file_signatures_reports_what_the_checker_inferred() {
    // The buffer is never loaded, which is exactly why hover cannot answer this and
    // the form-based inference can.
    let sigs = signatures("(defn f (s) (string/length s))");
    assert_eq!(sigs.len(), 1, "{sigs:?}");
    assert_eq!(sigs[0].0, "f");
    assert!(sigs[0].1.contains("string"), "{sigs:?}");
    assert!(!sigs[0].2, "an inferred sig must not read as declared");
}

#[test]
fn file_signatures_prefer_and_mark_a_declaration() {
    let sigs = signatures("(sig f (int -> string))\n(defn f (n) \"x\")");
    assert_eq!(sigs.len(), 1, "{sigs:?}");
    assert!(sigs[0].2, "a declared sig must be marked: {sigs:?}");
    assert!(sigs[0].1.contains("int"), "{sigs:?}");
}

#[test]
fn file_signatures_covers_guarded_clauses_and_skips_non_functions() {
    // A multi-clause definition has one signature per clause (ADR-261); the first is
    // what a reader is looking at.
    let sigs = signatures(
        "(defn g ((x) :when (string? x) 1) ((x) :when (int? x) 2))\n\
         (def not-a-fn 5)\n\
         (defmacro m (x) x)",
    );
    let names: Vec<&str> = sigs.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, vec!["g"], "{sigs:?}");
    assert!(sigs[0].1.contains("string"), "{sigs:?}");
    // …and it carries the union of the clauses' returns rather than a bare `any`.
    assert!(sigs[0].1.contains("1 | 2"), "{sigs:?}");
}

#[test]
fn file_signatures_reads_pattern_clauses() {
    // A PATTERN-dispatched multi-clause `defn` lowers to one variadic `fn` over `match*`,
    // which used to read as an arity-0 `(-> any)`; the un-expanded clauses say what each
    // position admits (a non-empty list pattern is `pair`, a literal its own type) and
    // the position-wise union over the arms is the signature a reader sees.
    let sigs = signatures(
        "(defn token
           (((x y & acc) \"+\") (conj acc (+ y x)))
           (((x y & acc) \"-\") (conj acc (- y x)))
           ((acc val) (conj acc (string/->number val))))",
    );
    assert_eq!(sigs.len(), 1, "{sigs:?}");
    // `acc` is not `any`: every clause body hands it to `conj`, whose declared domain now
    // flows BACKWARDS into the parameter. That narrowing is the point of giving `conj` a
    // signature, so it is pinned rather than relaxed — a change to `conj`'s domain has to
    // come past this line on purpose.
    assert_eq!(
        sigs[0].1, "(nil | pair | vector | map | set, string) -> any",
        "{sigs:?}"
    );
    // Two arms that agree on a position keep it: a literal head pins its type. The return
    // stays `any` — `conj` is declared `-> any`, so its result says nothing flat; it is the
    // call-site specialization test below that types it from what a combinator hands over.
    let sigs = signatures("(defn op ((acc \"+\") (conj acc 1)) ((acc \"-\") (conj acc 2)))");
    assert_eq!(
        sigs[0].1, "(nil | pair | vector | map | set, string) -> any",
        "{sigs:?}"
    );
}

#[test]
fn a_named_same_file_callback_flows_its_result_under_the_combinators_inputs() {
    // The reducer's own signature is `(any string -> any)` — nothing in its body
    // constrains `acc` — so its flat return said nothing. `reduce` knows the accumulator
    // it hands over (`'()`), and re-typing the body under that (call-site specialization,
    // what an inline `fn` always got) makes the fold precise.
    let sigs = signatures(
        "(defn step (acc val) (conj acc (string/->number val)))
         (defn run (xs) (first (reduce xs '() step)))",
    );
    let run = sigs.iter().find(|s| s.0 == "run").expect("run");
    assert_eq!(run.1, "(seqable) -> nil | number | failure", "{sigs:?}");
    // …and through a pattern-dispatched reducer, whose arms are chosen per input: the
    // list-pattern arms cannot admit the empty accumulator, the plain one can.
    let sigs = signatures(
        "(defn token
           (((x y & acc) \"+\") (conj acc (+ y x)))
           ((acc val) (conj acc (string/->number val))))
         (defn calc (input) (first (reduce (string/split input \" \") '() token)))",
    );
    let calc = sigs.iter().find(|s| s.0 == "calc").expect("calc");
    // No `nil`: `string/split` never returns an empty list (`""` splits to `("")`), so the
    // fold ran its step at least once and `first` has an element — `run` above keeps its
    // `nil` because a `seqable` may be empty.
    assert_eq!(calc.1, "(string) -> number | failure", "{sigs:?}");
    // `map` with a named callback flows its return the same way.
    let sigs = signatures(
        "(defn g (v) (string/->number v))
(defn h (xs) (first (map xs g)))",
    );
    let h = sigs.iter().find(|s| s.0 == "h").expect("h");
    assert_eq!(h.1, "(seqable) -> nil | number | failure", "{sigs:?}");
}

#[test]
fn an_inline_pattern_clause_callback_flows_its_result_too() {
    // The SAME clauses as the named `token` above, written INLINE in the reduce call.
    // The expander lowers a pattern-clause literal to one variadic `match*` lambda
    // before call sites are typed, so this read as `any` while the named spelling was
    // precise — two spellings of one program, two answers. The surface pre-pass records
    // the literal's clauses keyed by its printed head list; `clause_lambda_ret` recovers
    // them through the `[:match-error … (quote heads)]` datum the lowering embeds.
    let sigs = signatures(
        "(defn calc (input)
           (first (reduce (string/split input \" \") '()
             (fn (((x y & acc) \"+\") (conj acc (+ y x)))
                 ((acc val) (conj acc (string/->number val)))))))",
    );
    let calc = sigs.iter().find(|s| s.0 == "calc").expect("calc");
    // No `nil`: `string/split` never returns an empty list (`""` splits to `("")`), so the
    // fold ran its step at least once and `first` has an element — `run` above keeps its
    // `nil` because a `seqable` may be empty.
    assert_eq!(calc.1, "(string) -> number | failure", "{sigs:?}");
    // A single-clause literal with a destructuring parameter lowers the same way.
    let sigs = signatures("(defn firsts (pairs) (map pairs (fn ([k v]) k)))");
    let firsts = sigs.iter().find(|s| s.0 == "firsts").expect("firsts");
    assert_ne!(firsts.1, "(seqable) -> any", "{sigs:?}");
    // A variadic literal must NOT be typed positionally: `(fn (& args) …)` binding `&`
    // and `args` as two parameters is the bug that masked the recovery above.
    let sigs = signatures("(defn v (xs) (first (reduce xs '() (fn (& args) args))))");
    let v = sigs.iter().find(|s| s.0 == "v").expect("v");
    assert_eq!(v.1, "(seqable) -> any", "{sigs:?}");
}

#[test]
fn a_pass_through_parameter_specializes_at_the_call_site() {
    // `wrap` is `(any -> any)` on its own; a call hands it an int and gets one back.
    let sigs = signatures(
        "(defn wrap (x) (io/puts x) x)
(defn w () (wrap 3))",
    );
    let w = sigs.iter().find(|s| s.0 == "w").expect("w");
    assert_eq!(w.1, "() -> 3", "{sigs:?}");
}

#[test]
fn a_declared_callback_signature_is_honoured_by_the_combinators() {
    // A `(sig …)` on the callback used to buy nothing under `map`: the callback path
    // consulted only the loaded image. The same-file declaration is authoritative.
    let sigs = signatures(
        "(sig g (string -> int))
(defn g (v) (string/length v))
         (defn h (xs) (first (map xs g)))",
    );
    let h = sigs.iter().find(|s| s.0 == "h").expect("h");
    assert_eq!(h.1, "(seqable) -> nil | int", "{sigs:?}");
}

#[test]
fn specialization_declines_a_lexical_local_callback_and_a_variadic_arm() {
    // A `let`-bound callback is not the global of that name (the guard rails
    // `reduce_fold_bail_when_init_or_callback_unknown` pins), and a variadic arm has no
    // positional binding to specialize under — both stay flat.
    let sigs = signatures(
        "(defn step (acc val) (conj acc val))
         (defn run (xs) (let (step (fn (a v) a)) (reduce xs '() step)))
         (defn vstep (acc & vals) (conj acc (count vals)))
         (defn vrun (xs) (reduce xs '() vstep))",
    );
    let run = sigs.iter().find(|s| s.0 == "run").expect("run");
    assert!(!run.1.contains("list"), "{sigs:?}");
    let vrun = sigs.iter().find(|s| s.0 == "vrun").expect("vrun");
    assert!(vrun.1.ends_with("-> any"), "{sigs:?}");
}

#[test]
fn file_signatures_costs_nothing_when_unarmed() {
    // The capture is a no-op on the ordinary checking path — pinned because it runs
    // inside `check_file`, which every diagnostic request goes through.
    assert!(file_warnings("(defn f (s) (string/length s))")
        .iter()
        .all(|w| !w.contains("panic")));
    let ws = file_warnings("(defn f (s) (string/length s))\n(defn c () (f 5))");
    assert!(ws.iter().any(|w| w.contains("expects string")), "{ws:?}");
}

#[test]
fn a_module_private_function_is_inferred_and_its_call_sites_are_checked() {
    // `defn-` expands to `(do (def name (fn …)) (%mark-private 'name))`, so every pass
    // keyed on a top-level `(def …)` used to see NO definition at all for a private
    // function — and most definitions in a real module are private (40 of
    // `std/json.blsp`'s 42). The consequence was silent: their call sites went
    // unchecked, in exactly the internals where an argument-order slip lives.
    let warnings = file_warnings(
        r#"
        (defmodule privacy-demo)
        (defn- widen (s) (string/length s))
        (defn use-it () (widen 42))
        "#,
    );
    assert!(
        warnings.iter().any(|w| w.contains("expects string")),
        "a wrong-typed call to a private function must be flagged; got {warnings:?}"
    );
}

#[test]
fn a_macros_generated_temporary_is_never_typed() {
    // Two guards, one property. The descent into a top-level `do` is limited to the
    // `defn-`/`def-` expansion, and a gensym'd name is never typed regardless.
    // Opening a top-level `do` reaches more than `defn-`: other macros emit one over
    // GENERATED names. The linear-map rewrite wraps a fold's result in
    // `(do (def linmap-out__N …) …)`, and typing that temporary made the checker flag a
    // branch of the rewrite's own wrapper that cannot run with that value — a warning
    // naming a symbol the author never wrote and cannot fix.
    // `tests/linmap_soundness_test.blsp` caught it live; this pins the rule.
    let generated = file_warnings(
        r#"
        (do (def helper__42 (fn (s) (string/length s))) (io/puts "generated"))
        (defn consume () (helper__42 3))
        "#,
    );
    assert!(
        !generated.iter().any(|w| w.contains("expects string")),
        "a gensym'd definition must not be typed — nobody can act on the warning; got \
         {generated:?}"
    );

    // The descent that reaches it is narrow for a second reason: `defability`/`defimpl`
    // define their ops inside a top-level `do` too, and an INFERRED signature for an op
    // displaces the `:-> T` return the ability declares. Opening every `do` stopped that
    // return flowing to call sites — which is what
    // `ability_op_return_type_flows_to_call_site` pins.
}

#[test]
fn a_failed_equality_test_narrows_the_else_branch_for_a_literal() {
    // The tagged-union dispatch every Brood program writes: after `(= tag :ok)` fails,
    // a `(or :ok :err)` tag is `:err`. This needed the literal complement to be exact —
    // `¬:ok` used to widen to `any`, so the else branch learned nothing and the guard
    // was one-sided (`then_only`).
    let w = file_warnings(
        r#"
        (defn describe (tag)
          (if (%eq tag :ok) "fine" (string/length tag)))
        (sig describe ((or :ok :err) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains(":err")),
        "the else branch should know `tag` is `:err`, got {w:?}"
    );

    // …and the narrowing must not over-claim: a value the guard says nothing about
    // stays unnarrowed. `of_value` leaves a string literal flat (no heap for the
    // bytes), so `(= m "x")` proves only `m : string` and its negation proves nothing.
    let w = warnings(r#"(if (%eq m "x") :yes (string/length m))"#);
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "a non-literal guard type must stay one-sided: {w:?}"
    );
}

#[test]
fn a_record_shape_survives_keys_vals_assoc_and_dissoc() {
    // Closed records (ADR-264) made these sinks load-bearing rather than nice-to-have:
    // without them a closed record degrades to a flat `map` on its first update, and the
    // idiom that builds one field at a time loses its shape immediately.
    //
    // `assoc` adds the field it definitely puts there…
    let w = file_warnings(
        r#"
        (defn widen (r) (string/length (get (assoc r :count 1) :count)))
        (sig widen ((record :name string) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("1")),
        "assoc should carry the shape forward with :count added, got {w:?}"
    );

    // …`dissoc` removes it, so reading it back is `nil`…
    let w = file_warnings(
        r#"
        (defn drop-it (r) (string/length (get (dissoc r :count) :count)))
        (sig drop-it ((record :name string :count int) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("nil")),
        "dissoc should remove :count, so reading it yields nil, got {w:?}"
    );

    // …`keys` on a CLOSED record yields exactly the declared names…
    let w = file_warnings(
        r#"
        (defn ks (r) (string/length (first (keys r))))
        (sig ks ((record :name string :count int) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains(":count")),
        "keys should be the declared keyword literals, got {w:?}"
    );

    // …and `vals` the union of the declared field types. (A record with a `string`
    // field would NOT be flagged: the argument check fires on provable disjointness,
    // and a union containing `string` is not disjoint from it.)
    let w = file_warnings(
        r#"
        (defn vs (r) (string/length (first (vals r))))
        (sig vs ((record :count int :flag bool) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("int")),
        "vals should union the declared field types, got {w:?}"
    );
}

#[test]
fn an_open_record_declines_the_exhaustive_sinks() {
    // `keys`/`vals` read "these are ALL the keys", which is only true of a closed
    // record. An open one may carry keys nothing declares, so it must fall through to
    // the flat rule rather than claim a set it cannot know.
    let w = file_warnings(
        r#"
        (defn ks (r) (string/length (first (keys r))))
        (sig ks ((record &open :name string) -> any))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an open record's keys are not exhaustively known: {w:?}"
    );
}

#[test]
fn assoc_widens_the_map_refinement_to_what_it_adds() {
    // `(assoc m :extra "text")` on a `(map keyword int)` genuinely holds a string at
    // `:extra`. Carrying `K`/`V` forward unchanged claimed otherwise, so reading the key
    // back gave `nil | int` and flagged correct code — a false positive on the one
    // operation everyone uses to build a map.
    let w = file_warnings(
        r#"
        (defn widen (m) (string/length (get (assoc m :extra "text") :extra)))
        (sig widen ((map keyword int) -> any))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "assoc must widen V to include the value it adds: {w:?}"
    );

    // The refinement still narrows what it can: adding an int keeps the value type
    // `int`, so a string read out of it is still flagged.
    let w = file_warnings(
        r#"
        (defn keep (m) (string/length (get (assoc m :extra 1) :extra)))
        (sig keep ((map keyword int) -> any))
        "#,
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "adding an int must not widen V away: {w:?}"
    );
}

#[test]
fn a_parameter_in_call_head_position_is_callable() {
    // The callback shape. Without this a higher-order function's function parameter
    // types as `any`, so passing the sequence first — the classic argument-order slip —
    // is accepted in silence.
    let w = file_warnings(
        r#"
        (defn each-of (f xs) (f (first xs)))
        (defn misuse () (each-of 5 [1 2 3]))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("each-of") && s.contains("argument 1")),
        "a non-callable passed where the body calls it should be flagged: {w:?}"
    );

    // Callable is not just `fn`: a keyword is a function of a map, so passing one must
    // stay silent. (Maps, vectors and strings are NOT callable — each raises — so they
    // are correctly excluded.)
    let w = file_warnings(
        r#"
        (defn lookup (f m) (f m))
        (defn use-it () (lookup :a {:a 1}))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("lookup")),
        "a keyword IS callable on a map: {w:?}"
    );
}

#[test]
fn an_arrow_parameter_describes_the_call_it_heads() {
    // A `(sig …)` may declare a parameter's full signature, and the call inside the body
    // is the only site that can use it. It was inert in BOTH directions: the result had
    // no type and the arguments went unchecked, so declaring an arrow bought nothing.

    // The result flows…
    let w = file_warnings(
        r#"
        (sig apply-it ((int -> string) -> any))
        (defn apply-it (f) (+ 1 (f 1)))
        "#,
    );
    assert!(
        w.iter().any(|s| s.contains("string")),
        "the call's result should be the arrow's return type: {w:?}"
    );

    // …and the arguments are checked against the arrow's parameters.
    let w = file_warnings(
        r#"
        (sig apply-it ((int -> string) -> any))
        (defn apply-it (f) (f "not-an-int"))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("argument 1") && s.contains("expects int")),
        "the argument should be checked against the arrow's parameter: {w:?}"
    );

    // A correct use stays silent — this is the false-positive direction, and the
    // whole `std/` + `tests/` corpus is the wider version of this assertion.
    let w = file_warnings(
        r#"
        (sig apply-it ((int -> string) -> any))
        (defn apply-it (f) (string/length (f 1)))
        "#,
    );
    assert!(
        w.iter()
            .all(|s| !s.contains("apply-it") && !s.contains("string/length")),
        "a correct use of an arrow parameter must be silent: {w:?}"
    );

    // A local shadowing a global with an arrow type wins — `ctx.get` answers only for a
    // variable in scope, and it is consulted before the declared global.
    let w = file_warnings(
        r#"
        (sig helper (string -> int))
        (defn helper (s) (string/length s))
        (sig shadow ((int -> string) -> any))
        (defn shadow (helper) (helper 1))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("shadow")),
        "the shadowing local's arrow, not the global's sig, describes the call: {w:?}"
    );
}

#[test]
fn what_a_declared_parameter_catches_and_what_it_deliberately_does_not() {
    // Argument checks fire on **provable disjointness**, not on subtyping (ADR-110's
    // gradual relation: a precise argument is checked with `⊆`, a dynamic one with
    // `∩ ≠ ⊥`). That is easy to over- or under-trust in both directions — this pins the
    // line so neither happens by accident again.
    let src = |body: &str| {
        format!("(sig want ((record :a int) -> any))\n(defn want (r) r)\n(defn use-it () {body})")
    };
    let flags = |body: &str| {
        file_warnings(&src(body))
            .iter()
            .any(|w| w.contains("want") && w.contains("argument 1"))
    };

    // A CLOSED record catches all four ways a map can be the wrong shape — including
    // the two that only closedness (ADR-264) makes provable: a missing field, and an
    // extra one.
    assert!(flags("(want {:b 1})"), "wrong field name");
    assert!(flags("(want {:a \"s\"})"), "wrong field type");
    assert!(flags("(want {})"), "missing a required field");
    assert!(
        flags("(want {:a 1 :b 2})"),
        "an extra field — closed means closed"
    );
    assert!(!flags("(want {:a 1})"), "the right shape must be silent");

    // What it deliberately does NOT catch: an argument whose type is a UNION that
    // *might* be right. `(or int string)` is not disjoint from `string`, so a
    // `string` parameter accepts it. Widening rather than warning is the whole
    // gradual bargain — a warning here would fire on correct code whenever the
    // checker knows less than the programmer.
    let w = file_warnings(
        r#"
        (sig want-s (string -> any))
        (defn want-s (s) s)
        (sig give ((record :a int :b string) -> any))
        (defn give (r) (want-s (first (vals r))))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("want-s")),
        "a union that may be a string must not be flagged: {w:?}"
    );

    // …but a union with NO string in it is disjoint, and is flagged. The line is
    // "could this value possibly be right", not "is this value provably right".
    let w = file_warnings(
        r#"
        (sig want-s (string -> any))
        (defn want-s (s) s)
        (sig give ((record :a int :b bool) -> any))
        (defn give (r) (want-s (first (vals r))))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("want-s") && s.contains("expects string")),
        "a union that cannot be a string must be flagged: {w:?}"
    );
}

#[test]
fn a_body_that_never_returns_is_consistent_with_any_declared_return() {
    // `never` is disjoint from everything — an empty set shares no value with any set,
    // *itself included* — so the dynamic half of the return check read an always-throwing
    // body as inconsistent with whatever was declared. Every function that raises
    // unconditionally and carries a `(sig …)` drew a false positive, and at its silliest
    // one declared `never` was told its `never` body was wrong. It stayed hidden because
    // so few signatures were declared; adopting them across `std/` surfaced it.
    let w = file_warnings(
        r#"
        (sig boom (int -> string))
        (defn boom (n) (error "nope"))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("declared return type")),
        "an always-throwing body returns nothing, so it contradicts no declaration: {w:?}"
    );

    // …including when the declaration is `never` itself.
    let w = file_warnings(
        r#"
        (sig boom (int -> never))
        (defn boom (n) (error "nope"))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("declared return type")),
        "`never` against `never`: {w:?}"
    );

    // The check still fires on a body that DOES return something disjoint.
    let w = file_warnings(
        r#"
        (sig wrong (int -> string))
        (defn wrong (n) 5)
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("declared return type string") && s.contains("yields 5")),
        "a real mismatch must still be reported: {w:?}"
    );
}

#[test]
fn a_file_local_declaration_constrains_its_callers_in_that_file() {
    // A `(sig …)` is on `ctx`, not in the heap — the file is checked before it loads —
    // so the domain walk's "callee with a known signature" rule, which read the heap,
    // could not see it. A declaration constrained callers in every OTHER module and not
    // in its own, which is where most calls to it are: `std/bytes.blsp` declares
    // `(sig at (bytes int -> int))` and calls `(bytes/at bs off)` three lines later.
    let w = file_warnings(
        r#"
        (defn at (bs i) bs)
        (sig at (bytes int -> int))
        (defn read-at (bs off) (at bs off))
        (defn misuse () (read-at "not-bytes" "not-an-int"))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("read-at") && s.contains("argument 1")),
        "the callee's declared parameter types should reach its caller's domain: {w:?}"
    );

    // …and a lexical local shadowing the name is NOT the declared global, so it imposes
    // nothing — the same guard every other declared-sig lookup carries.
    let w = file_warnings(
        r#"
        (defn at (bs i) bs)
        (sig at (bytes int -> int))
        (defn shadowed (at x) (at x x))
        (defn fine () (shadowed (fn (a b) a) 1))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("shadowed: argument")),
        "a shadowing local must not inherit the global's declared parameters: {w:?}"
    );
}
