//! Refinement that flows through the checker: map key/value types, record annotations and fields, overloads (same file and cross-module), literal returns.

use super::*;

#[test]
fn map_kv_refinement_flows_through_checker() {
    // (sig f ((map keyword int) -> int)): the get result is int | nil.
    // Feeding that to string-length should warn. Without the sig the result
    // type is unknown → no warning, so the sig must be declared — use
    // file_warnings so the `sig` form is parsed.
    let src = "
(defn f (m) (get m :k))
(sig f ((map keyword int) -> int))
(string/length (f {:a 1}))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for int|nil arg, got {w:?}"
    );

    // `(keys m)` where m : map<keyword, int> → nil | list<keyword>.
    // Feeding to string-length warns (list is not a string).
    let src2 = "
(defn g (m) (keys m))
(sig g ((map keyword int) -> (list keyword)))
(string/length (g {:a 1}))
";
    let w2 = file_warnings(src2);
    assert!(
        w2.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for list<keyword> arg, got {w2:?}"
    );

    // Correct uses stay silent.
    for ok in [
        "(get {:a 1} :a)", // any map get — flat result, no warning
        "(keys {:a 1})",
        "(vals {:a 1})",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn record_type_annotation_parses_and_accepts_valid_calls() {
    // `(record …)` is accepted as a `(sig …)` annotation and carries a
    // full field refinement (see docs/type-records.md), so a valid call
    // produces no spurious warning.
    let src = "
(defn f (m) m)
(sig f ((record :a int :b (optional string)) -> any))
(f {:a 1 :b \"x\"})
";
    let w = file_warnings(src);
    assert!(w.is_empty(), "expected no warnings, got {w:?}");

    // A malformed record annotation (odd field-list length, or a
    // non-keyword key) is dropped rather than guessed — the sig source
    // still parses (it's just not read as an authoritative signature),
    // so the checker doesn't crash and falls back to no declared sig.
    for bad in [
        "(defn f (m) m)\n(sig f ((record :a int :b) -> any))\n(f {:a 1})",
        "(defn f (m) m)\n(sig f ((record a int) -> any))\n(f {:a 1})",
    ] {
        let _ = file_warnings(bad); // must not panic
    }
}

#[test]
fn record_field_refinement_flows_through_checker() {
    // (sig f ((record :a int) -> int)): `(get m :a)` on a declared record
    // resolves to the *exact field type* (int | nil), not a flat
    // fallback — feeding that to string-length should warn.
    let src = "
(defn f (m) (get m :a))
(sig f ((record :a int) -> int))
(string/length (f {:a 1}))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for int|nil arg, got {w:?}"
    );

    // A key a *fully-read literal* doesn't carry is provably absent (ADR-264), so it
    // reads as `nil` — and `(string/length nil)` is a real error, not a false positive.
    assert!(
        warnings("(let (m {:a 1}) (string/length (get m :other)))")
            .iter()
            .any(|w| w.contains("expects string")),
        "an absent key on a closed literal is nil, and must be caught"
    );
    // But a literal the checker could not read completely stays OPEN — the dropped
    // entry might be the key being asked for, so nothing may be concluded about it.
    assert!(
        warnings("(let (m {:a 1 :b (unknown-thing)}) (string/length (get m :other)))")
            .iter()
            .all(|w| !w.contains("expects string")),
        "an incompletely-read literal must not claim a key is absent"
    );

    // Record-literal type inference: `{:a 1}` infers a record shape
    // (`:a` required, type int) directly from the literal, no `sig`
    // needed — feeding the field straight to a sink warns.
    assert!(
        warnings("(string/length (get {:a 1} :a))")
            .iter()
            .any(|w| w.contains("string/length")),
        "expected a warning from the inferred record-literal shape"
    );

    // Correct uses stay silent.
    for ok in [
        "(get {:a 1} :a)",
        "(string/length (get {:a \"x\"} :a))",
        "(get {:a 1} :b)", // undeclared key — unresolved, not a warning
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn overload_refinement_flows_through_checker() {
    // (sig f (and (int -> int) (string -> string))): `f`'s return type
    // depends on which arm matched the call's argument (ADR-116).
    let src = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(string/length (f 1))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "an int arg should resolve to the int arm's return type, got {w:?}"
    );

    // The string arm's return type feeds `string-length` cleanly.
    let src2 = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(string/length (f \"hi\"))
";
    assert!(
        file_warnings(src2).is_empty(),
        "a string arg should resolve to the string arm's return type, got {:?}",
        file_warnings(src2)
    );

    // The string arm's return type is NOT a number — feeding it to `+` warns.
    let src3 = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(+ 1 (f \"hi\"))
";
    assert!(
        file_warnings(src3).iter().any(|s| s.contains('+')),
        "a string return type fed to + should warn, got {:?}",
        file_warnings(src3)
    );

    // An argument of unknown type widens to the union of every matching
    // arm's return — `int | string` — which is NOT disjoint from
    // `string`, so no false positive (sound, just less precise).
    let src4 = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(defn g (y) (string/length (f y)))
";
    assert!(
        file_warnings(src4).is_empty(),
        "an unknown-typed arg should widen, not warn, got {:?}",
        file_warnings(src4)
    );
}

#[test]
fn overload_resolves_cross_module_via_the_heap_store() {
    // `file_warnings`/`warnings` never *evaluate* — `%register-sig` only
    // runs at load time, so those helpers only ever exercise the
    // per-file `Ctx` path (`ctx.declared_overload`), never the
    // heap-level `runtime.declared_sigs` store that makes a plain
    // single-arrow sig visible cross-module. Simulate "module A defines
    // and declares f; module B (a fresh Ctx — no file-local knowledge of
    // f at all) calls it" by actually *evaluating* the declaration first
    // (`eval_str`, so `%register-sig` really populates the heap), then
    // typing a call form against an empty `Ctx` — exactly module B's
    // starting point.
    use super::infer::expr_ty;

    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
",
        )
        .expect("module A loads cleanly");

    // An int-typed argument resolves to the int arm's return type.
    let call_int = reader::read_one(&mut interp.heap, "(f 1)").expect("parse");
    let t = expr_ty(&interp.heap, call_int, &Ctx::default())
        .expect("cross-module overload should resolve, not come back unknown");
    assert!(
        t.is_subtype(&Ty::of(Tag::Int)),
        "expected int for an int arg, got {t}"
    );

    // A string-typed argument resolves to the string arm's return type.
    let call_str = reader::read_one(&mut interp.heap, "(f \"hi\")").expect("parse");
    let t2 = expr_ty(&interp.heap, call_str, &Ctx::default())
        .expect("cross-module overload should resolve, not come back unknown");
    assert!(
        t2.is_subtype(&Ty::of(Tag::Str)),
        "expected string for a string arg, got {t2}"
    );
}

#[test]
fn int_literal_return_type_flows_through_checker() {
    // (sig f ((or 200 404 500) -> …)): f's declared return type is an
    // int-literal set (ADR-117), not flat `int` — feeding a call to
    // `string-length` should warn (disjoint tags: string vs. int), the
    // same as it would for a flat `int` return, proving the literal-set
    // Ty flows through `sig_of`/`declared_sig` like any other arrow.
    let src = "
(defn f (x) x)
(sig f ((or 200 404 500) -> (or 200 404 500)))
(string/length (f 200))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for an int-literal return, got {w:?}"
    );

    // A correct use (an int sink) stays silent.
    let src2 = "
(defn f (x) x)
(sig f ((or 200 404 500) -> (or 200 404 500)))
(+ 1 (f 200))
";
    assert!(
        file_warnings(src2).is_empty(),
        "an int-literal return fed to + should be silent, got {:?}",
        file_warnings(src2)
    );
}
