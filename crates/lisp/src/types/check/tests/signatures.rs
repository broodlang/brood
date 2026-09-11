//! Signatures as the checker reads them: curated closures, `(sig …)` declarations (optional/tuple/keyword params), dead clauses under a typed param, the curated helper table.

use super::*;

#[test]
fn flags_literal_misuse_of_primitives() {
    // An int literal now infers as its singleton (B0), so the diagnostic
    // names the exact value (`5`) rather than the coarse `int` tag.
    assert!(warnings("(first 5)")
        .iter()
        .any(|w| w.contains("first") && w.contains("got 5")));
    // A keyword literal now infers as its singleton type, so the diagnostic
    // names the exact value (`:k`) rather than the coarse `keyword` tag.
    assert!(warnings("(string/length :k)")
        .iter()
        .any(|w| w.contains("string/length") && w.contains(":k")));
    assert!(warnings("(%add 1 \"x\")")
        .iter()
        .any(|w| w.contains("%add")));
    assert!(warnings("(%vector-ref [1 2] :k)")
        .iter()
        .any(|w| w.contains("vector-ref")));
}

#[test]
fn no_false_positives_when_type_is_unknown_or_right() {
    assert!(warnings("(first (list 1 2))").is_empty()); // arg is a non-sig call → dynamic
    assert!(warnings("(first xs)").is_empty()); // variable → dynamic
    assert!(warnings("(first [1 2 3])").is_empty()); // vector is allowed
    assert!(warnings("(%add 1 2)").is_empty());
    assert!(warnings("(string/length \"hi\")").is_empty());
}

#[test]
fn propagates_primitive_result_types() {
    // string-length returns int; first wants a list/vector → flag the int.
    assert!(warnings("(first (string/length \"a\"))")
        .iter()
        .any(|w| w.contains("first") && w.contains("int")));
}

#[test]
fn an_any_result_is_not_a_false_positive() {
    // `%vector-ref` on a vector whose elements are unknown is unknown, so feeding it to
    // string-length (wants string) must NOT warn — `any` overlaps `string`.
    assert!(warnings("(string/length (%vector-ref xs 0))").is_empty());
    // …but on a literal it is exactly that position (a destructuring `let` lowers to
    // `%vector-ref` behind a length check, so this is what types `[x y] rect` binders).
    assert!(warnings("(string/length (%vector-ref [1] 0))")
        .iter()
        .any(|w| w.contains("string/length")));
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
    // map's SECOND argument must be callable; an int is not (data-first, ADR-308).
    assert!(warnings("(map xs 1)")
        .iter()
        .any(|w| w.contains("map") && w.contains("argument 2")));
    // Correct uses, and an unknown (variable) callable, stay silent.
    assert!(warnings("(+ 1 2)").is_empty());
    assert!(warnings("(map xs inc)").is_empty()); // inc is a variable → unknown
}

#[test]
fn sig_declaration_is_read_by_the_checker() {
    // A user (sig …) gives a branchy fn a signature the checker trusts:
    // arguments checked against the declared params.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x) (if (> x 0) x (- x)))\n(f \"s\")");
    assert!(
        w.iter()
            .any(|m| m.contains("f:") && m.contains("argument 1") && m.contains("int")),
        "declared param type should flag (f \"s\"): {w:?}"
    );
    // The declared *result* flows out: f : int, string-length wants string.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x) x)\n(string/length (f 3))");
    assert!(
        w.iter().any(|m| m.contains("string/length")),
        "declared result type should flag string/length: {w:?}"
    );
    // Correct uses stay silent.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x) x)\n(f 3)\n(+ 1 (f 4))");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "correct uses of a declared fn must be silent: {w:?}"
    );
}

#[test]
fn keyword_literal_types_in_a_sig_are_enforced() {
    // A parameter typed as an enumerated keyword set flags a keyword outside it.
    let w = file_warnings("(sig f ((or :a :b) -> int))\n(defn f (x) 1)\n(f :c)");
    assert!(
        w.iter()
            .any(|m| m.contains("f:") && m.contains("argument 1") && m.contains(":a | :b")),
        "a keyword outside the literal set should flag, naming it: {w:?}"
    );
    // A member of the set is fine.
    let w = file_warnings("(sig f ((or :a :b) -> int))\n(defn f (x) 1)\n(f :a)");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a keyword in the set must be silent: {w:?}"
    );
    // The declared literal *result* flows out and is checked too.
    let w = file_warnings(
            "(sig mode (-> (or :maximized :fullscreen)))\n(defn mode () :maximized)\n(string/length (mode))",
        );
    assert!(
        w.iter().any(|m| m.contains("string/length")),
        "a keyword-literal result feeding string-length should flag: {w:?}"
    );
}

#[test]
fn sig_declaration_handles_arity_unions_and_bad_exprs() {
    // Arity comes from the declared param count for a file-local defn the
    // read-only checker can't otherwise inspect.
    let w = file_warnings("(sig g (int int -> int))\n(defn g (a b) (+ a b))\n(g 1)");
    assert!(
        w.iter().any(|m| m.contains("expected 2")),
        "declared arity should flag (g 1): {w:?}"
    );
    // Union result type: (or int nil) — feeding it to a sink that wants a
    // string is still a provable mismatch.
    let w = file_warnings("(sig h (int -> (or int nil)))\n(defn h (x) x)\n(string/length (h 1))");
    assert!(
        w.iter().any(|m| m.contains("string/length")),
        "union result (int|nil) is disjoint from string: {w:?}"
    );
    // An unparseable type-expr is dropped — never a false signal.
    let w = file_warnings("(sig k (bogus -> int))\n(defn k (x) x)\n(k \"s\")");
    assert!(
        w.iter()
            .all(|m| !m.contains("k:") || !m.contains("argument")),
        "an unrecognised type-expr must be ignored, not guessed: {w:?}"
    );
}

#[test]
fn variadic_defn_with_sig_does_not_get_a_false_arity_warning() {
    // Regression: the `(sig …)` parser only builds *fixed*-arity sigs, so a
    // sig on a **variadic** defn would record an exact arity equal to the
    // declared param count. A read-only whole-file check can't inspect the
    // real (unevaluated) closure, so it falls back to that count — and a call
    // with more args than the sig lists would falsely warn. The def site's
    // own `& rest` must suppress the sig-derived exact arity.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x & rest) x)\n(f 1 2 3)");
    assert!(
        w.iter()
            .all(|m| !(m.contains("f:") && m.contains("number of arguments"))),
        "a variadic defn must not get a false arity warning: {w:?}"
    );
    // `&rest` spelling, and below the declared count is fine too.
    let w = file_warnings("(sig g (int int -> int))\n(defn g (a &rest more) a)\n(g 1 2 3 4)");
    assert!(
        w.iter()
            .all(|m| !(m.contains("g:") && m.contains("number of arguments"))),
        "&rest variadic defn must not get a false arity warning: {w:?}"
    );
    // A multi-arity fn with a variadic arm is likewise variadic.
    let w = file_warnings("(sig h (int -> int))\n(defn h ((x) x) ((x & ys) x))\n(h 1 2 3)");
    assert!(
        w.iter()
            .all(|m| !(m.contains("h:") && m.contains("number of arguments"))),
        "multi-arity variadic defn must not get a false arity warning: {w:?}"
    );
    // Control: a *fixed*-arity sig'd defn STILL gets its arity checked (the
    // fix must not over-suppress) — mirrors the case above.
    let w = file_warnings("(sig p (int int -> int))\n(defn p (a b) (+ a b))\n(p 1)");
    assert!(
        w.iter().any(|m| m.contains("expected 2")),
        "fixed-arity sig'd defn must still be arity-checked: {w:?}"
    );
}

#[test]
fn optional_sig_params_parse_and_check() {
    // `&optional` in `(sig …)` grammar — previously unsupported: the whole
    // arrow silently failed to parse (no marker recognized `&optional`,
    // so `parse_type` on that symbol returned `None`, propagating out
    // through `parse_arrow`), meaning the sig vanished with zero warning
    // at all, not just an unchecked optional slot.

    // Call-site: the optional argument's declared type is checked, same
    // as a required one.
    let w =
        file_warnings("(sig g (int &optional string -> int))\n(defn g (a &optional b) a)\n(g 1 2)");
    assert!(
        w.iter()
            .any(|m| m.contains("g: argument 2 expects string") && m.contains("got 2")),
        "an optional arg's declared type must be checked: {w:?}"
    );

    // Arity: calling with just the required arg, or with the optional
    // one supplied, is fine; one too many is an arity error.
    let w =
        file_warnings("(sig g (int &optional string -> int))\n(defn g (a &optional b) a)\n(g 1)");
    assert!(
        w.iter().all(|m| !m.contains("number of arguments")),
        "omitting an optional arg must not warn: {w:?}"
    );
    let w = file_warnings(
        r#"(sig g (int &optional string -> int))
(defn g (a &optional b) a)
(g 1 "x")"#,
    );
    assert!(
        w.iter()
            .all(|m| !m.contains(", got ") && !m.contains("expects string")),
        "supplying the optional arg with the right type must not warn: {w:?}"
    );
    let w = file_warnings(
        "(sig g (int &optional string -> int))\n(defn g (a &optional b) a)\n(g 1 \"x\" 2)",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("expected 1 to 2 arguments, got 3")),
        "one arg beyond required+optional must still be an arity error: {w:?}"
    );

    // Body seeding: an optional param is widened with `nil` (it may
    // genuinely be absent), so a defensive `(nil? b)` check is never
    // mistaken for dead code the way an exact required-param contract
    // would be — but real misuse (using it unconditionally as if it
    // can't be nil) is still caught.
    let w = file_warnings(
        "(sig g (int &optional string -> int))\n\
             (defn g (a &optional b) (if (nil? b) a (+ a (string/length b))))",
    );
    assert!(
        w.is_empty(),
        "a defensive nil-check on an optional param must not warn: {w:?}"
    );
    let w =
        file_warnings("(sig g (int &optional string -> int))\n(defn g (a &optional b) (+ a b))");
    assert!(
        w.iter()
            .any(|m| m.contains("+: argument 2 expects number") && m.contains("nil | string")),
        "using an optional param unconditionally as non-nil must still warn: {w:?}"
    );

    // `&optional` combined with a trailing `&` rest, mirroring a
    // closure's full `(req &optional opt & rest)` shape.
    let w = file_warnings(
            "(sig h (int &optional string & number -> int))\n(defn h (a &optional b & c) a)\n(h 1 \"x\" true)",
        );
    assert!(
        w.iter()
            .any(|m| m.contains("h: argument 3 expects number") && m.contains("got true")),
        "a rest arg after an optional one must still be checked: {w:?}"
    );

    // Malformed order (`&` before `&optional`) is still never *misparsed* into
    // something incorrect — but since Pass 2.85 the author is told, rather than
    // the declaration vanishing silently (an annotation that is ignored when
    // wrong is a gate that cannot fail). Nothing about `k` itself is checked.
    let w = file_warnings("(sig k (int & number &optional string -> int))\n(defn k (a) a)");
    assert!(
        w.iter()
            .any(|m| m.contains("sig k: malformed function type")),
        "a malformed marker order must be reported: {w:?}"
    );
    assert!(
        w.iter().all(|m| !m.contains("k: argument")),
        "…and must not be misparsed into an argument check: {w:?}"
    );
}

#[test]
fn tuple_sig_params_parse_and_check() {
    // `(tuple T1 T2 …)` — a fixed-arity positional vector shape
    // (ADR-128). A vector *literal* infers its exact per-position types
    // (not a widened uniform element type), so a mismatched literal
    // argument is caught by the ordinary disjointness check — no new
    // machinery needed at the call site itself.
    let w = file_warnings("(sig f ((tuple int string) -> any))\n(defn f (t) t)\n(f [\"x\" 1])");
    assert!(
        w.iter()
            .any(|m| m.contains("f: argument 1 expects (tuple int, string)")
                && m.contains("got (tuple \"x\", 1)")),
        "a mismatched tuple-shaped literal argument must warn: {w:?}"
    );
    let w = file_warnings("(sig f ((tuple int string) -> any))\n(defn f (t) t)\n(f [1 \"x\"])");
    assert!(
        w.is_empty(),
        "a matching tuple-shaped literal must not warn: {w:?}"
    );

    // Different arity is disjoint too (a vector has one definite length).
    let w =
        file_warnings("(sig f ((tuple int string) -> any))\n(defn f (t) t)\n(f [1 \"x\" true])");
    assert!(
        w.iter().any(|m| m.contains("f: argument 1")),
        "a wrong-arity tuple literal must warn: {w:?}"
    );

    // Position-aware `first`/`second`/`third`/`last`/`nth` on a
    // tuple-typed param: each resolves to its *exact* position's type
    // (not the coarse union every other element access falls back to),
    // so a mismatch on the specific position used is caught.
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (first t)))",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("string/length: argument 1 expects string") && m.contains("int")),
        "first on a tuple must resolve to position 0's exact type: {w:?}"
    );
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (second t)))",
    );
    assert!(
        w.is_empty(),
        "second on this tuple is already a string — no warning: {w:?}"
    );
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (nth t 0)))",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("string/length: argument 1 expects string") && m.contains("int")),
        "a literal-index nth on a tuple must resolve position-exactly: {w:?}"
    );
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (nth t 1)))",
    );
    assert!(
        w.is_empty(),
        "nth at the string position must not warn: {w:?}"
    );

    // Return-type flow: a tuple-shaped return type is checked against
    // the body's inferred literal shape, same as any other declared
    // return type.
    let w = file_warnings(
        r#"(sig f (-> (tuple int string)))
(defn f () ["x" 1])"#,
    );
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type (tuple int, string)")),
        "a mismatched declared tuple return type must warn: {w:?}"
    );

    // A tuple is a subtype of the corresponding uniform vector type (every
    // element of a `tuple<int,string>` is an `int | string`) — so passing
    // a tuple-shaped literal where a plain `(vector …)` is expected must
    // not warn just because the shapes differ.
    let w = file_warnings("(sig g ((vector any) -> any))\n(defn g (v) v)\n(g [1 \"x\"])");
    assert!(
        w.is_empty(),
        "a tuple literal must satisfy a uniform vector param: {w:?}"
    );
}

#[test]
fn dead_clause_flagged_for_a_sig_typed_param() {
    // A `match` literal pattern that can't match the parameter's declared type.
    let w =
        file_warnings("(sig f (int -> keyword))\n(defn f (n) (match n (\"hi\" :s) (_ :other)))");
    assert!(
        w.iter()
            .any(|m| m.contains("unreachable clause") && m.contains("int")),
        "a string-literal clause when n : int should be dead: {w:?}"
    );
    // A `cond` predicate disjoint from the declared parameter type.
    let w = file_warnings("(sig g (int -> keyword))\n(defn g (n) (cond (string? n) :s else :o))");
    assert!(
        w.iter().any(|m| m.contains("unreachable clause")),
        "(string? n) when n : int should be dead: {w:?}"
    );
}

#[test]
fn dead_clause_silent_without_sig_or_when_compatible_or_a_literal_scrutinee() {
    // No `sig` → the parameter is untyped → never flagged (no false positive).
    assert!(
        file_warnings("(defn k (n) (match n (\"hi\" :s) (_ :o)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "no sig ⇒ no dead-clause"
    );
    // A recognised but *compatible* guard narrows, it isn't dead.
    assert!(
        file_warnings("(sig h (int -> keyword))\n(defn h (n) (cond (int? n) :i else :o))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "(int? n) when n : int must not flag"
    );
    // A literal scrutinee is not a sig-typed param — the gate excludes it (this
    // is the intentional non-match test shape that the naive lint flagged).
    assert!(
        file_warnings("(defn m () (match [1 2] ((a) :one) (_ :o)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a literal scrutinee must never be flagged dead"
    );
}

#[test]
fn dead_clause_flagged_for_a_precise_let_local() {
    // ADR-131: the dead-clause lint now covers a *precise, surface* `let`-local,
    // not just a sig-typed param. `x` is statically `5` (a literal, precise), so
    // a `string?` clause can never run.
    let w = file_warnings("(defn f () (let (x 5) (cond (string? x) :a else :b)))");
    assert!(
        w.iter()
            .any(|m| m.contains("unreachable clause") && m.contains("string")),
        "a string? clause on a let-local typed 5 should be dead: {w:?}"
    );
    // A `match` literal pattern disjoint from the local's type is dead too.
    let w = file_warnings("(defn g () (let (x 5) (match x (\"hi\" :s) (_ :o))))");
    assert!(
        w.iter().any(|m| m.contains("unreachable clause")),
        "a string-literal pattern on a let-local typed int should be dead: {w:?}"
    );
}

#[test]
fn dead_clause_let_local_respects_precision_gensym_and_compatibility() {
    // A *compatible* guard narrows without emptying — not dead.
    assert!(
        file_warnings("(defn a () (let (x 5) (cond (int? x) :a else :b)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "(int? x) when x : int must not flag"
    );
    // A local bound to a **call result** is `dynamic` (redefinable → the type
    // could change on reload), so it's excluded — no dead-clause warning even
    // though the current-image type would narrow to `never`. Reload-safe.
    assert!(
        file_warnings("(defn h () 5)\n(defn b () (let (x (h)) (cond (string? x) :a else :b)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a call-result local is dynamic and must not be flagged dead"
    );
    // A **gensym** temporary (macro-introduced) is exempt — warning on a name
    // the user can't rename would be noise.
    assert!(
        file_warnings("(defn c () (let (x__1 5) (cond (string? x__1) :a else :b)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a gensym let-local must never be flagged dead"
    );
    // Shadowing a precise local with an unknown one drops eligibility.
    assert!(
        file_warnings("(defn d (y) (let (x 5) (let (x y) (cond (string? x) :a else :b))))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a shadowing rebind of unknown type must not be flagged dead"
    );
}

#[test]
fn curated_helper_sigs_catch_misuse() {
    // even?/odd?/abs require a number. Written QUALIFIED: they are `math/` since ADR-227,
    // and the bare spelling is now unbound without `(:use math)`. Asserting the bare form
    // here was testing a dead key — and worse, the bare entry it relied on is what
    // suppressed the unbound lint on a name that no longer exists (see `sigs.rs`).
    assert!(warnings("(math/even? \"x\")")
        .iter()
        .any(|w| w.contains("even?") && w.contains("number")));
    assert!(warnings("(math/odd? :k)")
        .iter()
        .any(|w| w.contains("odd?") && w.contains("number")));
    assert!(warnings("(math/abs :k)")
        .iter()
        .any(|w| w.contains("abs") && w.contains("number")));
    // count wants a string | map | sequence, not a number.
    assert!(warnings("(count 5)").iter().any(|w| w.contains("count")));
    // There is no `length` function and never was (see `sigs.rs`) — so the right warning
    // is UNBOUND, not a type mismatch. Asserted explicitly, because the old assertion
    // ("some warning mentioning length") passes either way and so proved nothing.
    assert!(warnings("(length :k)")
        .iter()
        .any(|w| w.contains("unbound") && w.contains("length")));
    // not/zero? accept any arg but pin a bool *result*, so feeding it to a
    // numeric sink is caught (the result-type payoff).
    assert!(warnings("(+ 1 (not x))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    assert!(warnings("(+ 1 (math/zero? x))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    // Correct uses stay silent (no false positives).
    for ok in [
        "(math/even? 4)",
        "(math/abs -3)",
        "(count [1 2 3])",
        "(count \"hi\")",
        // `bytes` is seqable/countable: these iterate its octets at runtime.
        "(count (bytes 1 2 3))",
        "(first (bytes 1 2 3))",
        "(rest (bytes 1 2 3))",
        "(every? (bytes 1 3 5) math/odd?)",
        "(not x)",
        "(math/zero? n)",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

/// A curated sig must never be the reason a name looks *bound*.
///
/// An entry in `CURATED_SIGS` marks its name as one the checker knows, which suppresses the
/// unbound lint — so a stale entry for a name that has moved out of the prelude makes
/// `nest check` silent on code that dies at runtime. That is what happened after ADR-227
/// moved `even?`/`odd?`/`abs` into `std/math.blsp`: a bare `(even? 4)` with no `(:use math)`
/// is an unbound error when run, and `nest check` — the gate that exits nonzero on any
/// warning — reported nothing at all, while its uncurated siblings `sum`/`frequencies`
/// correctly said "unbound symbol". Nothing else in the tree observes this: the checker is
/// advisory, so the only symptom is a program that passes CI and then fails.
#[test]
fn a_curated_sig_does_not_mask_the_unbound_lint_for_a_moved_name() {
    for moved in ["even?", "odd?", "abs", "index-where"] {
        let src = format!("(defn f () ({moved} 4))");
        let ws = file_warnings(&src);
        assert!(
            ws.iter()
                .any(|w| w.contains("unbound") && w.contains(moved)),
            "bare `{moved}` is `math/`/`seq/` since ADR-227 and unbound without an import, \
             so the checker must say so — a curated sig keyed on the bare name silences this \
             and lets a runtime-unbound program pass `nest check`. Got: {ws:?}"
        );
    }
    // The qualified spelling is the one that exists, and it still gets the vetted signature.
    assert!(warnings("(math/abs :k)")
        .iter()
        .any(|w| w.contains("abs") && w.contains("number")));
}

#[test]
fn curated_output_and_numeric_sigs() {
    // io/puts and io/write return nil — feeding to a numeric sink is caught.
    for f in ["io/puts", "io/write"] {
        let w = warnings(&format!("(+ 1 ({f} \"hi\"))"));
        assert!(
            w.iter().any(|s| s.contains('+') && s.contains("nil")),
            "{f}: expected '+' nil-result warning, got {w:?}"
        );
    }
    // min/max require at least one number.
    assert!(warnings("(math/min \"a\" 2)")
        .iter()
        .any(|w| w.contains("min") && w.contains("number")));
    assert!(warnings("(math/max 1 :k)")
        .iter()
        .any(|w| w.contains("max") && w.contains("number")));
    // min/max return a number — feeding to a string sink is caught.
    assert!(warnings("(string/length (math/min 1 2))")
        .iter()
        .any(|w| w.contains("string/length")));
    // Correct uses stay silent.
    for ok in [
        "(io/puts \"hi\")",
        "(math/min 1 2 3)",
        "(math/max 0.5 1.5)",
        "(+ 1 (math/min 2 3))",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}
