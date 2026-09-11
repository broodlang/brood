//! The gradual checks (ADR-078/110): callback arity, `(def x …)` against a value sig, declared return types, precise body inference, path narrowing, overload calls, typed globals.

use super::*;

// ---- callback-arity check over higher-order combinators (ADR-078) ----

#[test]
fn flags_a_named_callback_of_the_wrong_arity() {
    // `cons` is arity 2; `map` calls its callback with 1 arg → real bug.
    let w = warnings("(map nil cons)");
    assert!(
        w.iter()
            .any(|s| s.contains("map") && s.contains("callback") && s.contains("cons")),
        "map should flag a 2-arg callback called with 1: {w:?}"
    );
}

#[test]
fn accepts_a_named_callback_of_the_right_arity() {
    // `inc` is arity 1 — exactly what `map` supplies. No warning.
    let w = warnings("(map nil inc)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a correct-arity callback must not warn: {w:?}"
    );
    // A variadic callback (`+` accepts 1) is fine too.
    let w = warnings("(map nil +)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a variadic callback must not warn: {w:?}"
    );
}

#[test]
fn flags_an_inline_fn_callback_of_the_wrong_arity() {
    // A 2-param inline fn passed where `map` calls it with 1 arg.
    let w = warnings("(map nil (fn (a b) a))");
    assert!(
        w.iter()
            .any(|s| s.contains("map") && s.contains("callback") && s.contains("the fn")),
        "map should flag a 2-arg fn: {w:?}"
    );
    // Correct arity — no warning.
    let w = warnings("(map nil (fn (a) a))");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a 1-arg fn must not warn under map: {w:?}"
    );
}

#[test]
fn lambda_is_retired_and_hints_at_fn() {
    // ADR-162 retired the alias: `fn` is the only spelling. `lambda` is now an
    // ordinary unbound name — with a hint naming `fn`, so the mistake is one line to
    // fix. (It was a synonym for years, claimed removed by the docs for months.)
    let w = warnings("(map nil (lambda (a b) a))");
    assert!(
        w.iter().any(|s| s.contains("lambda")),
        "a `lambda` head must be reported now: {w:?}"
    );
    assert_eq!(
        crate::eval::foreign_construct_hint("lambda"),
        Some("Brood spells `lambda` as `fn`: `(fn (x) …)`.")
    );
    // The `fn` spelling still gets the callback-arity check it always did.
    let w = warnings("(map nil (fn (a b) a))");
    assert!(
        w.iter()
            .any(|s| s.contains("map") && s.contains("callback")),
        "map should flag a 2-arg `fn` callback: {w:?}"
    );
}

#[test]
fn fn_form_is_not_unbound() {
    // Regression (originally found via the `lambda` alias, retired in ADR-162): a fn
    // head missing from SPECIAL_HEAD / is_syntactic_keyword made whole-file mode flag
    // the head AND its params as unbound — a false positive on valid code.
    let w = file_warnings("(def f (map (list 1 2 3) (fn (x) (+ x 1))))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol")),
        "an `fn` literal must not draw unbound-symbol warnings: {w:?}"
    );
}

// ---- gradual-assignment check: `(def x …)` vs a non-arrow `(sig x T)` ----
// (GradualTy's first consumer — ADR-024.)

#[test]
fn def_against_value_sig_flags_a_literal_mismatch() {
    // `(sig n int)` then `(def n "hello")` — a precise literal disjoint from
    // the declared type. stat(string) ⊄ int → flagged.
    let w = file_warnings(r#"(sig n int) (def n "hello")"#);
    assert!(
        w.iter().any(|m| m.contains("n: value of type \"hello\"")
            && m.contains("not assignable")
            && m.contains("int")),
        "a string literal assigned to an int-declared name must warn: {w:?}"
    );
}

#[test]
fn def_against_value_sig_catches_a_bounded_dynamic_global() {
    // The genuine GradualTy value-add: `label` is a redefinable global with a
    // declared type, so it's dynamic_within(string) — a bounded dynamic that
    // Option<Ty> can't represent. Assigning it to an int-declared name is
    // disjoint (string ∩ int = ⊥) → flagged.
    let w =
        file_warnings(r#"(sig count int) (sig label string) (def label "x") (def count label)"#);
    assert!(
        w.iter()
            .any(|m| m.contains("count: value of type string") && m.contains("int")),
        "a string-typed global assigned to an int-declared name must warn: {w:?}"
    );
}

#[test]
fn def_against_value_sig_defers_when_consistent_or_unknown() {
    // Every one of these is consistent (or dynamic) → no assignment warning.
    for src in [
        "(sig n int) (def n 5)",                          // exact
        "(sig m number) (def m 5)",                       // int <: number
        "(sig n int) (def n (+ 1 2))",                    // call result widened → defer
        "(sig n int) (def n some-unknown-global)",        // unknown → pure dynamic
        "(sig a int) (sig b number) (def b 5) (def a b)", // int <- number: ∩≠⊥ → defer
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("not assignable")),
            "a consistent/dynamic assignment must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn value_sig_resolves_cross_module_via_the_heap_store() {
    // Same technique as `overload_resolves_cross_module_via_the_heap_store`,
    // for the *value-type* `(sig name T)` declaration instead of an arrow:
    // `file_warnings`/`warnings` never evaluate, so they only ever exercise
    // the per-file `Ctx` path (`ctx.declared_value_ty`), never the heap-wide
    // `declared_sigs` store that makes a plain value sig visible cross-module
    // (`sigs::declared_heap_value_ty`). Simulate "module A declares `label`
    // and `count`; module B (fresh `Ctx`, no file-local knowledge of either)
    // assigns `count`'s value from `label`" by actually *evaluating* the
    // declarations first, then checking the `(def …)` form against an empty
    // `Ctx` — module B's starting point.
    let mut interp = crate::Interp::new();
    interp
        .eval_str(r#"(sig label string) (def label "x") (sig count int)"#)
        .expect("module A loads cleanly");

    let form = reader::read_one(&mut interp.heap, "(def count label)").expect("parse");
    let w = check_form(&interp.heap, form);
    assert!(
        w.iter()
            .any(|m| m.contains("count: value of type string") && m.contains("int")),
        "a string-typed global (declared cross-module) assigned to an \
             int-declared name (declared cross-module) must warn: {w:?}"
    );

    // And the consistent case: assigning a `string`-declared name from
    // `label` must stay silent, proving this isn't just an always-warn bug.
    interp
        .eval_str("(sig other string)")
        .expect("module A extension loads cleanly");
    let form2 = reader::read_one(&mut interp.heap, "(def other label)").expect("parse");
    let w2 = check_form(&interp.heap, form2);
    assert!(
        w2.iter().all(|m| !m.contains("not assignable")),
        "a string-typed global assigned to a string-declared name must not \
             warn: {w2:?}"
    );
}

#[test]
fn defmodule_declared_arrow_sig_seeds_return_type_check() {
    // Regression: `(sig f (-> B))` declared inside a `defmodule` block
    // didn't seed `check_def`'s body-vs-declared-return-type check.
    // Pass 2.5 (`annot::parse_sig_decl`) records a declared sig under the
    // symbol exactly as written in the un-expanded `(sig …)` form — bare
    // `f`. But `defn f` inside a `defmodule` expands to
    // `(def mod/f (fn …))`, so `check_def`'s seeding lookup
    // (`ctx.declared_sig(name)`) looks up the *qualified* `mod/f`, which
    // never matches the bare-keyed entry — the sig silently never seeds.
    // Needs `%register-sig` to have actually run (real `eval`, not just
    // parse+check) for the heap-wide fallback to have anything to read,
    // so this uses the same real-`Interp` + `eval_str` technique as the
    // cross-module tests, then re-checks the same source as a whole file
    // (mirrors what `nest check` does on an already-loaded project).
    let src = r#"
(defmodule gap-check-test-mod "doc")
(sig gap-check-test-f (-> string))
(defn gap-check-test-f ()
  "doc"
  42)
"#;
    let mut interp = crate::Interp::new();
    interp.eval_str(src).expect("module loads cleanly");

    let forms = reader::read_all(&mut interp.heap, src).expect("parse");
    let w = check_file(&mut interp.heap, &forms);
    assert!(
        w.iter()
            .any(|(_, m)| m.contains("gap-check-test-mod/gap-check-test-f")
                && m.contains("declared return type string")
                && m.contains("yields 42")),
        "a defmodule-qualified defn's body vs its declared return type \
             must warn, same as at the root namespace: {w:?}"
    );
}

#[test]
fn cross_module_value_sig_dependency_is_captured_for_incremental_cache() {
    // Regression for a gap the ADR-119 Phase 2 merge surfaced: `sigs::
    // declared_heap_value_ty` (ADR-124) originally read `heap.
    // declared_sig_value` directly instead of through `deps::
    // obs_declared_sig_value` — the *only* sanctioned read of global state
    // Phase 2's incremental-cache dependency capture relies on.
    //
    // Specifically isolates `check_def`'s own gate (the *name being
    // defined*, not the value referenced): `other`'s sig lives only on
    // the heap (module A), never in this file's own text, so
    // `ctx.declared_value_ty("other")` is `None` and `check_def` must
    // fall through to `declared_heap_value_ty` to know `other`'s type at
    // all. `other` never appears as a *value reference* anywhere in this
    // file (it's purely a def target), so — unlike a referenced global —
    // nothing else (the unbound-symbol check, arity lookups, …) would
    // incidentally record it via `deps::obs_global` either. If
    // `declared_heap_value_ty` bypasses the recorder, `other` never
    // enters this file's dep-keys at all, and a later edit to its sig is
    // invisible to the fingerprint — exactly the bug this guards.
    let mut interp = crate::Interp::new();
    interp
        .eval_str(r#"(sig label string) (def label "x") (sig other int)"#)
        .expect("module A loads cleanly");

    let forms = reader::read_all(&mut interp.heap, "(def other label)").expect("parse");
    let (warnings, dep_keys) = check_file_with_deps(&mut interp.heap, &forms);
    assert!(
        warnings
            .iter()
            .any(|(_, m)| m.contains("other: value of type string") && m.contains("int")),
        "int-declared `other` assigned a string must warn even with no \
             local (sig other …): {warnings:?}"
    );
    let fp1 = deps_fingerprint(&interp.heap, dep_keys);

    // Module A is "edited": other's declared type widens to accept a
    // string. This file's fingerprint must change — `other` is never
    // referenced here, only defined, so this changed fact can only reach
    // the fingerprint through check_def's own heap-wide lookup.
    interp
        .eval_str("(sig other string)")
        .expect("module A edit loads cleanly");
    let fp2 = deps_fingerprint(&interp.heap, dep_keys);
    assert_ne!(
        fp1, fp2,
        "a cross-module value-sig change on a pure def-target global must \
             flip the dependent file's fingerprint, or the incremental cache \
             would go stale"
    );
}

#[test]
fn declared_return_type_mismatch_is_flagged() {
    // Body yields an int (the integer-closed `+` rule: `int + int = int`),
    // declared return is string → disjoint → flagged.
    let w = file_warnings("(sig f (int -> string)) (defn f (x) (+ x 1))");
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type string") && m.contains("yields int")),
        "an int body vs a string return must warn: {w:?}"
    );
    // A literal body mismatch too.
    let w = file_warnings(r#"(sig g (int -> int)) (defn g (x) "hello")"#);
    assert!(
        w.iter()
            .any(|m| m.contains("g: declared return type int") && m.contains("\"hello\"")),
        "a string-literal body vs an int return must warn: {w:?}"
    );
}

#[test]
fn sig_call_site_wrong_literal_arg_is_flagged() {
    // A literal argument whose type is disjoint from the parameter's
    // declared `(sig …)` type is flagged at the call site (the precise `⊆`
    // path — a string literal where an int is wanted).
    let w = file_warnings(r#"(sig f (int -> int)) (defn f (x) x) (f "hello")"#);
    assert!(
        w.iter()
            .any(|m| m.contains("f: argument 1 expects int") && m.contains("\"hello\"")),
        "a string literal passed where int is declared must warn: {w:?}"
    );
    // A correct literal, and a dynamic (non-literal) argument, must not warn.
    for src in [
        "(sig g (int -> int)) (defn g (x) x) (g 1)",
        "(sig h (int -> int)) (defn h (x) x) (defn use-h (y) (h y))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("argument 1 expects")),
            "a consistent/dynamic argument must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn record_arg_missing_optional_field_does_not_warn() {
    // A record value that omits an *optional* field is a valid argument — the
    // arg-check relaxes the param to its required fields only, so the missing
    // `:age` (declared `(optional int)`) never misfires.
    let decl = "(sig f ((record :name string :age (optional int)) -> int)) (defn f (r) 0)";
    for good in ["(f {:name \"Ada\"})", "(f {:name \"Ada\" :age 30})"] {
        let w = file_warnings(&format!("{decl} {good}"));
        assert!(
            w.iter().all(|m| !m.contains("argument 1 expects")),
            "a record arg omitting an optional field must not warn ({good}): {w:?}"
        );
    }
    // But a wrong-typed *required* field is still caught (the sound part the
    // optional-drop preserves).
    let w = file_warnings(&format!("{decl} (f {{:name 42}})"));
    assert!(
        w.iter().any(|m| m.contains("f: argument 1 expects")),
        "a record arg with a wrong-typed required field must warn: {w:?}"
    );
}

#[test]
fn check_allow_type_mismatch_suppresses_call_and_return_lints() {
    // `(check-allow :type-mismatch …)` opts a deliberately-wrong subtree out
    // of BOTH the call-site argument lint and the declared-return lint —
    // the negative-test escape hatch (a `sig!` runtime contract is what the
    // wrapped code actually exercises).
    let w = file_warnings(
        r#"(sig f (int -> int)) (defn f (x) x) (check-allow :type-mismatch (f "hello"))"#,
    );
    assert!(
        w.iter().all(|m| !m.contains("argument 1 expects")),
        "check-allow :type-mismatch must suppress the call-site arg lint: {w:?}"
    );
    // The sig stays at top level (pass 2.5 reads sigs from top-level forms);
    // only the deliberately-wrong defn is wrapped — the contract_test shape.
    let w =
        file_warnings(r#"(sig g (int -> int)) (check-allow :type-mismatch (defn g (x) "nope"))"#);
    assert!(
        w.iter().all(|m| !m.contains("return type")),
        "check-allow :type-mismatch must suppress the return-type lint: {w:?}"
    );
}

#[test]
fn wider_sig_param_returned_as_narrower_is_flagged() {
    // A sig-typed param carries its exact contract type, so returning a
    // `number` param where the declared return is `int` is caught via the
    // precise `⊆` path — the first non-disjoint ("merely wider") mismatch the
    // disjointness checker structurally can't produce.
    let w = file_warnings("(sig f (number -> int)) (defn f (x) x)");
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type int") && m.contains("number")),
        "a number param returned as int must warn: {w:?}"
    );
    // Same or narrower param, and a param narrowed by a guard, must not warn.
    for src in [
        "(sig g (int -> int)) (defn g (x) x)",
        "(sig h (int -> number)) (defn h (x) x)",
        "(sig k (number -> int)) (defn k (x) (if (int? x) x 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "a consistent/narrowed param return must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn declared_return_type_defers_when_consistent() {
    // (+ x 1) : number — int <: number and number ∩ int ≠ ⊥, so neither of
    // these declared returns warns (a widened body never over-warns).
    for src in [
        "(sig inc (int -> int)) (defn inc (x) (+ x 1))",
        "(sig h (int -> number)) (defn h (x) (+ x 1))",
        "(sig id (int -> int)) (defn id (x) x)",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "a consistent return must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn precise_body_inference_int_closed_ops() {
    // The "int int thing": `(* x x)` with `x : int` is precisely `int`, so a
    // body declared `(int -> int)` must NOT warn (the false-positive flood the
    // curated `number` result would otherwise produce).
    let w = file_warnings("(sig f (int -> int)) (defn f (x) (* x x))");
    assert!(
        w.iter().all(|m| !m.contains("return type")),
        "`(* int int)` declared int must not warn: {w:?}"
    );
    // A real lie still warns: an int body declared `string`.
    let w = file_warnings("(sig f (int -> string)) (defn f (x) (* x 2))");
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type string") && m.contains("yields int")),
        "an int body declared string must warn: {w:?}"
    );
}

#[test]
fn precise_body_inference_float_contagion() {
    // Float-contagion: `+ - * /` with a provably-float operand is precisely
    // `float` (int⊕float → float in the tower), and the always-float unary math
    // `sqrt`/`sin`/`cos`/`tan` is `float` even for a whole-number argument. Since
    // `float` is disjoint from `int`, a body declared `(int -> int)` doing float
    // arithmetic warns — the merely-wider mismatch the flat `number` sig missed.
    for src in [
        "(sig f (int -> int)) (defn f (x) (+ x 1.5))",
        "(sig f (int -> int)) (defn f (x) (* x 2.0))",
        "(sig f (int -> int)) (defn f (x) (math/sqrt x))",
        "(sig f (int -> int)) (defn f (x) (/ x 2.0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("f: declared return type int") && m.contains("yields float")),
            "a float body declared int must warn ({src}): {w:?}"
        );
    }
    // Sound-defer cases that must NOT warn: a float body declared `float` or
    // `number`, an all-int body (int-closed rule), and `/` on two ints — which
    // is genuinely `number` (`(/ 6 2)` → 3, `(/ 5 2)` → 2.5), so it can't be
    // pinned to `float` and defers rather than false-positive.
    for src in [
        "(sig f (int -> float)) (defn f (x) (+ x 1.5))",
        "(sig f (int -> number)) (defn f (x) (* x 2.0))",
        "(sig f (int -> int)) (defn f (x) (* x x))",
        "(sig f (int -> int)) (defn f (x) (/ x 2))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "a consistent/deferred float-arithmetic body must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn path_narrowing_through_a_record_field_guard() {
    // `(if (int? (get r :age)) …)` narrows the *path* `(get r :age)` to `int`
    // in the then-branch — so feeding it to `string-length` (wants string) is
    // caught, the miss occurrence typing on bare symbols couldn't reach.
    let w = file_warnings("(defn f (r) (if (int? (get r :age)) (string/length (get r :age)) 0))");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("got int")),
        "an int-narrowed path fed to string-length must warn: {w:?}"
    );
    // A **nested** path narrows too: `(get (get cfg :db) :port)`.
    let nested = file_warnings(
        "(defn n (cfg) (if (int? (get (get cfg :db) :port)) \
             (string/length (get (get cfg :db) :port)) 0))",
    );
    assert!(
        nested
            .iter()
            .any(|m| m.contains("string/length") && m.contains("got int")),
        "an int-narrowed nested path must warn: {nested:?}"
    );
    // Uses consistent with the narrowed type — and an unguarded access (wide
    // type) — must NOT warn.
    for src in [
        "(defn g (r) (if (int? (get r :age)) (+ 1 (get r :age)) 0))",
        "(defn h (r) (if (string? (get r :n)) (string/length (get r :n)) 0))",
        "(defn m (r) (string/length (get r :age)))",
        // else-branch use of a `¬string`-narrowed path must not misfire.
        "(defn k (r) (if (string? (get r :x)) :s (get r :x)))",
        // a *different* nested path than the one narrowed must not warn.
        "(defn p (c) (if (int? (get (get c :db) :port)) (string/length (get (get c :web) :h)) 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a consistent/unguarded path use must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn path_narrowing_through_index_paths() {
    // `(nth t 0)` / `(first t)` / `(second …)` / `(third …)` narrow like a
    // field path: an int-narrowed index fed to `string-length` is caught.
    for src in [
        "(defn f (t) (if (int? (nth t 0)) (string/length (nth t 0)) 0))",
        "(defn f (t) (if (int? (first t)) (string/length (first t)) 0))",
        // mixed field + index path.
        "(defn f (r) (if (int? (nth (get r :xs) 0)) (string/length (nth (get r :xs) 0)) 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("string/length") && m.contains("got int")),
            "an int-narrowed index path must warn ({src}): {w:?}"
        );
    }
    // A *different* index than the one narrowed must not warn (index-specific),
    // and a consistent use must not warn.
    for src in [
        "(defn f (t) (if (int? (nth t 0)) (string/length (nth t 1)) 0))",
        "(defn f (t) (if (int? (nth t 0)) (+ 1 (nth t 0)) 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a different/consistent index use must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn path_narrowing_refines_base_record_type_into_calls() {
    // A path guard refines `base`'s *record type* in the then-branch, so it
    // flows into a call: `r` proven `{age: int}` passed where `{age: string}`
    // is wanted is caught (record disjointness on a conflicting required field).
    let decl = "(sig f ((record :age string) -> int)) (defn f (r) 0)";
    let bad = file_warnings(&format!(
        "{decl} (defn g (r) (if (int? (get r :age)) (f r) 0))"
    ));
    assert!(
        bad.iter()
            .any(|m| m.contains("f: argument 1 expects") && m.contains("got")),
        "a base refined to a conflicting record must warn at the call: {bad:?}"
    );
    // Matching field type, and an unguarded pass, must NOT warn.
    let okdecl = "(sig h ((record :age int) -> int)) (defn h (r) 0)";
    for src in [
        format!("{okdecl} (defn g (r) (if (int? (get r :age)) (h r) 0))"),
        format!("{okdecl} (defn g (r) (h r))"),
    ] {
        let w = file_warnings(&src);
        assert!(
            w.iter().all(|m| !m.contains("argument 1 expects")),
            "a matching/unguarded record arg must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn overload_call_matching_no_arm_is_flagged() {
    // (sig f (and (int -> int) (bool -> bool))): a call whose argument is
    // disjoint from *every* arm's domain is flagged (ADR-116 completion).
    let decl = "(sig f (and (int -> int) (bool -> bool))) \
                    (defn f (x) (if (int? x) (+ x 1) (not x)))";
    let bad = file_warnings(&format!(r#"{decl} (def c (f "hello"))"#));
    assert!(
        bad.iter()
            .any(|m| m.contains("f: no clause accepts these arguments")),
        "an arg matching no arm must warn: {bad:?}"
    );
    // An arg that matches *some* arm, and an unknown arg, must NOT warn.
    for src in [
        "(def a (f 5))",      // int → arm 1
        "(def b (f true))",   // bool → arm 2
        "(defn g (y) (f y))", // unknown arg → defer
    ] {
        let w = file_warnings(&format!("{decl} {src}"));
        assert!(
            w.iter().all(|m| !m.contains("no overload clause")),
            "a matching/unknown arg must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn check_allow_suppresses_targeted_lints() {
    // A `(check-allow :non-tail-recursion …)` wrapper silences the non-tail
    // lint for the wrapped defn — but only that category, and only inside it.
    let non_tail = "(defn f (n) (if (< n 1) 0 (+ 1 (f (- n 1)))))";
    let w = file_warnings(non_tail);
    assert!(
        w.iter().any(|m| m.contains("non-tail position")),
        "unwrapped non-tail recursion must warn: {w:?}"
    );
    let w = file_warnings(&format!("(check-allow :non-tail-recursion {non_tail})"));
    assert!(
        w.iter().all(|m| !m.contains("non-tail position")),
        "check-allow :non-tail-recursion must suppress: {w:?}"
    );
    // A mismatched category does NOT suppress (no silent blanket opt-out).
    let w = file_warnings(&format!("(check-allow :unreachable-clause {non_tail})"));
    assert!(
        w.iter().any(|m| m.contains("non-tail position")),
        "a mismatched category must not suppress the non-tail lint: {w:?}"
    );
    // Same for the redundant-`match`-clause lint.
    let dup = "(defn g (x) (match x (1 :a) (1 :b) (_ :z)))";
    assert!(
        file_warnings(dup)
            .iter()
            .any(|m| m.contains("unreachable clause")),
        "unwrapped duplicate clause must warn"
    );
    let wrapped = "(defn g (x) (check-allow :unreachable-clause (match x (1 :a) (1 :b) (_ :z))))";
    assert!(
        file_warnings(wrapped)
            .iter()
            .all(|m| !m.contains("unreachable clause")),
        "check-allow :unreachable-clause must suppress the redundancy lint"
    );
}

#[test]
fn precise_body_inference_control_flow() {
    // `(if (> x 0) x "neg")` yields `int | string`, which ⊄ int → must warn
    // (precise control-flow inference: both branches pin a type).
    let w = file_warnings(r#"(sig f (int -> int)) (defn f (x) (if (> x 0) x "neg"))"#);
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type int") && m.contains("\"neg\"")),
        "an `int | string` body declared int must warn: {w:?}"
    );
    // A branchy body that stays within the declared type must NOT warn.
    let w = file_warnings("(sig f (int -> int)) (defn f (x) (if (> x 0) x 0))");
    assert!(
        w.iter().all(|m| !m.contains("return type")),
        "an all-int branchy body declared int must not warn: {w:?}"
    );
}

#[test]
fn precise_body_inference_defers_on_uncertainty() {
    // A body ending in a call to an un-sig'd local/global is unknown → defer,
    // never warn (graceful degradation keeps the check false-positive-clean).
    for src in [
        // an un-sig'd file-global call
        "(defn helper (x) x) (sig f (int -> int)) (defn f (x) (helper x))",
        // an un-sig'd let-bound local call
        "(sig f (int -> int)) (defn f (x) (let (g (fn (y) y)) (g x)))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "an unknown-result body must defer ({src}): {w:?}"
        );
    }
}

#[test]
fn argument_check_uses_the_full_gradual_relation() {
    // Gating B1 (docs/type-gating.md): the arg check now runs the gradual
    // relation, so a *merely-wider precise* argument is caught (a `number`
    // sig-param passed where `int` is wanted) — closing the return/arg
    // asymmetry.
    let w = file_warnings(
        "(sig wants-int (int -> int)) (defn wants-int (n) n) \
             (sig f (number -> int)) (defn f (x) (wants-int x))",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("wants-int: argument 1 expects int") && m.contains("got number")),
        "a merely-wider precise argument must warn: {w:?}"
    );
    // But B0 keeps it sound: a literal argument is a faithful singleton, so
    // `200` passed where `(or 200 404 500)` is wanted does NOT false-positive.
    let w = file_warnings("(sig g ((or 200 404 500) -> int)) (defn g (c) c) (defn u () (g 200))");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a literal in the accepted set must not warn: {w:?}"
    );
    // And a *dynamic* argument (a call result) still defers on `∩` — only a
    // provably-disjoint one warns, never a merely-wider one.
    let w = file_warnings(
        "(sig produce (int -> number)) (defn produce (n) n) \
             (sig h (int -> int)) (defn h (n) n) (defn top () (h (produce 3)))",
    );
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a dynamic (call-result) argument must defer, not over-warn: {w:?}"
    );
}

#[test]
fn undeclared_global_current_type_gates_its_use() {
    // Gap A (docs/type-gating.md): an *undeclared* global defined exactly once
    // by `(def g 5)` gets its inferred current-image type (`int`), so misusing
    // it is caught — via `dynamic_within` (the `∩` relation), reload-safe.
    let w = file_warnings("(def g 5) (defn f () (string/length g))");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("got 5")),
        "an undeclared int global misused must warn: {w:?}"
    );
    // Consistent use, a redefined (ambiguous) global, and a function global
    // must NOT warn.
    for src in [
        "(def g 5) (defn f () (+ 1 g))", // int used as int
        "(def g 5) (def g \"s\") (defn f () (string/length g))", // redefined → dynamic
        "(defn g (x) x) (defn f () (+ 1 (g 2)))", // function global, not a value
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a consistent/ambiguous/function global must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn cross_file_undeclared_global_gates_via_loaded_image() {
    // Cross-file Gap A: an undeclared global defined in one place (loaded into
    // the image) is typed from its heap value where it's used elsewhere — the
    // same mechanism `infer_sig` uses for functions. `check_with_defs` evals
    // the def, then checks a separate form (the cross-context path).
    let w = check_with_defs(&["(def gg 5)"], "(string/length gg)");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("got 5")),
        "a cross-file undeclared int global misused must warn: {w:?}"
    );
    // A **dynamic variable** must be excluded — its heap value is only the
    // default; `binding` rebinds it to any type, so typing a use against the
    // default would false-positive. `(binding (*dv* "s") (string/length *dv*))`
    // is valid and must NOT warn.
    let w = check_with_defs(
        &["(defdyn *dv* 0)"],
        "(binding (*dv* \"s\") (string/length *dv*))",
    );
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a dynamic variable must stay unknown, not be typed from its default: {w:?}"
    );
    // A function global isn't gated as a value (its arrow is handled by sig_of).
    let w = check_with_defs(&["(defn ff (x) x)"], "(+ 1 ff)");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a function global must not be gated as a plain value: {w:?}"
    );
}

#[test]
fn declared_global_type_flows_into_value_position() {
    // `(sig g int)` makes `g`'s declared type visible where it's used, so a
    // disjoint use is caught — even though `g` is a redefinable global.
    let w = file_warnings("(sig g int) (def g 5) (def r (string/length g))");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("int")),
        "a declared int global used where a string is wanted must warn: {w:?}"
    );
    // A compatible use defers (int ⊆ number).
    let w = file_warnings("(sig g int) (def g 5) (def r (+ 1 g))");
    assert!(
        w.iter().all(|m| !m.contains("expects number")),
        "a declared int global is fine for +: {w:?}"
    );
}

#[test]
fn unknown_module_qualified_name_is_not_unbound() {
    // A qualified reference whose module isn't loaded — defined dynamically
    // (`%load-string`, a required temp module) or in a file a single-file check
    // didn't load — can't be proven unbound, so it's left alone.
    for src in [
        "(some-unloaded-mod/thing 1)",
        "(a/b/c/deep-thing 1)",
        "(+ 1 other-mod/value)",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("unbound symbol")),
            "an unknown-module qualified name must not be flagged ({src}): {w:?}"
        );
    }
    // But a typo in a *known* module (some `mod/*` is loaded) is still flagged:
    // requiring `io` makes `io/` a known prefix. `io` and not `test`, deliberately — a
    // lean (`--no-default-features`) runtime embeds no dev modules, so `(require-one 'test)`
    // resolves to nothing there and the assertion vanished with it. A CORE module keeps
    // the test about the checker instead of about the build's feature set.
    let w = file_warnings("(io/no-such-fn 1)");
    assert!(
        w.iter()
            .any(|m| m.contains("unbound symbol: io/no-such-fn")),
        "a typo in a known module must still be flagged: {w:?}"
    );
}

#[test]
fn ki17_qualified_reference_auto_requires_so_no_unrequired_warning() {
    // KI-17 is OBSOLETE since the ADR-227 follow-up: a qualified reference `mod/name`
    // now *infers* `(require-one 'mod)`, so "a reference to an unrequired module" can no
    // longer occur — there is no unrequired module to reference. The lint is a permanent
    // no-op, so a qualified reference draws NO "unrequired module" warning regardless of
    // the reachability set (empty or populated), and NO "unbound" (the reference resolves).
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defmodule ki17mod \"m\")\n(defn foo (x) x)")
        .expect("module loads");
    let forms = crate::syntax::reader::read_all(&mut interp.heap, "(defn go (x) (ki17mod/foo x))")
        .expect("parse");

    // Empty reachability set — once the flag for KI-17, now silent.
    let warned = crate::types::check::check_file_ext(&mut interp.heap, &forms, &[]);
    assert!(
        warned.iter().all(|(_, m)| !m.contains("unrequired module")),
        "KI-17 is obsolete — no unrequired-module warning is expected, got {warned:?}"
    );
    assert!(
        warned.iter().all(|(_, m)| !m.contains("unbound symbol")),
        "the qualified reference resolves — it is not 'unbound': {warned:?}"
    );

    // The module in the reachability set — also silent (unchanged).
    let ok =
        crate::types::check::check_file_ext(&mut interp.heap, &forms, &["ki17mod".to_string()]);
    assert!(
        ok.iter().all(|(_, m)| !m.contains("unrequired module")),
        "expected silence when the module is reachable, got {ok:?}"
    );
}

#[test]
fn ki17_alias_clause_feeds_the_require_closure() {
    // Regression (generative fuzzer find): `(:alias mod :as x)` *loads* `mod` (it
    // `require`s it, then adds the `x/` prefix), so `module_direct_requires` must report
    // `mod` as a direct dependency — else a file that `:alias`es a module and also names
    // it qualified (or via the alias, which macro-expands to `mod/…`) false-positives.
    let mut interp = crate::Interp::new();
    let forms = crate::syntax::reader::read_all(
        &mut interp.heap,
        "(defmodule c \"c\" (:use ua) (:use-internals ub) (:alias uc :as x))\n(defn use () 1)",
    )
    .expect("parse");
    let (own, deps) = crate::types::check::module_direct_requires(&interp.heap, &forms);
    assert_eq!(own.as_deref(), Some("c"));
    for m in ["ua", "ub", "uc"] {
        assert!(
            deps.iter().any(|d| d == m),
            "{m} should be a direct require (deps = {deps:?})"
        );
    }
}

#[test]
fn unexpandable_macro_calls_dont_false_flag() {
    // A file-local macro the checker can't expand: its arguments are opaque
    // syntax. (a) A macro that `def`s its symbol arg — the name must not look
    // unbound later. (b) A macro that splices an arg into a binder — the
    // spliced names must not look unbound.
    let a = file_warnings("(defmacro mk (n) `(def ~n (fn (x) x))) (mk qf) (qf 5)");
    assert!(
        a.iter().all(|m| !m.contains("unbound symbol")),
        "a macro-defined name must not look unbound: {a:?}"
    );
    let b = file_warnings("(defmacro wp (v & body) `(let ((a b) ~v) ~@body)) (wp [1 2] (+ a b))");
    assert!(
        b.iter().all(|m| !m.contains("unbound symbol")),
        "names a macro splices into a binder must not look unbound: {b:?}"
    );
    // A genuine typo under a *known* (arg-evaluating) callee is still flagged.
    let c = file_warnings("(io/puts (genuine-typo 5))");
    assert!(
        c.iter().any(|m| m.contains("unbound symbol: genuine-typo")),
        "a real unbound call head must still be flagged: {c:?}"
    );
}

#[test]
fn transient_is_a_valid_count_and_contains_arg() {
    // count/contains? dispatch to transient-* kernel hooks at runtime, so a live
    // transient is a valid argument — the sigs must admit Tag::Transient. (`length` was
    // listed here too and does not exist; see `sigs.rs`.)
    for src in ["(count (transient {}))", "(contains? (transient {}) :k)"] {
        let w = warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "transient must be accepted by {src}: {w:?}"
        );
    }
    // A genuinely wrong arg (a number) is still flagged — the domain stays tight.
    assert!(warnings("(count 5)").iter().any(|m| m.contains("count")));
}

#[test]
fn multi_arity_fn_clause_params_are_bound() {
    // Regression: `check_fn` read a multi-arity fn's first clause as a param
    // list, so a param used only in a *later* clause looked unbound — a false
    // positive.
    let w = file_warnings("(def g (fn ((a) (* a 2)) ((a b) (+ a b))))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol")),
        "multi-arity fn clause params must not look unbound: {w:?}"
    );
    // `defn` (which expands to `(def name (fn …))`) too.
    let w = file_warnings("(defn h ((a) a) ((a b) (+ a b)))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol")),
        "defn: {w:?}"
    );
}

#[test]
fn self_recursive_let_bound_closure_is_bound() {
    // Regression: a `let`-bound `fn`/`lambda` that calls its own binding name
    // resolves at runtime (the closure captures the frame, late-binds on call),
    // but the checker flagged the self-reference unbound. Pre-binding fn-valued
    // let names fixes it — for `let` and `let*`, `fn` and `lambda`.
    let w = file_warnings("(defn t () (let (fac (fn (n) (if (= n 0) 1 (fac n)))) (fac 5)))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol: fac")),
        "self-recursive let closure must not look unbound: {w:?}"
    );
    // But an *eager* forward reference in a non-closure RHS still surfaces.
    let w = file_warnings("(defn t () (let (a undefined-thing b 1) a))");
    assert!(
        w.iter()
            .any(|m| m.contains("unbound symbol: undefined-thing")),
        "an eager forward/undefined reference must still be flagged: {w:?}"
    );
}

#[test]
fn reduce_and_fold_expect_a_two_arg_callback() {
    // reduce/fold call `(f acc x)` — 2 args. A 1-arg callback is wrong.
    let w = warnings("(reduce nil 0 (fn (a) a))");
    assert!(
        w.iter()
            .any(|s| s.contains("reduce") && s.contains("callback")),
        "reduce should flag a 1-arg callback: {w:?}"
    );
    let w = warnings("(fold nil 0 inc)");
    assert!(
        w.iter()
            .any(|s| s.contains("fold") && s.contains("callback")),
        "fold should flag a 1-arg callback (inc): {w:?}"
    );
    // A correct 2-arg callback is silent.
    let w = warnings("(reduce nil 0 (fn (a b) a))");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a 2-arg callback must not warn under reduce: {w:?}"
    );
}

#[test]
fn callback_arity_is_skipped_when_unknown() {
    // A multi-arity lambda accepts 1 *and* 2 — must not warn (we bail rather
    // than risk a false positive).
    let w = warnings("(map nil (fn ((a) a) ((a b) a)))");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "multi-arity lambda must be skipped: {w:?}"
    );
    // A locally-bound callback has unknown arity here — skip.
    let w = warnings("(fn (f) (map nil f))");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a local callback must be skipped: {w:?}"
    );
}
