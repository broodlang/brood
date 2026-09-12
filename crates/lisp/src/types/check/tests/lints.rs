//! The advisory lints: non-tail self-recursion, literal misuse of primitives, unused bindings, error-testing forms, unbound symbols in literals, reversed argument order.

use super::*;

#[test]
fn flags_non_tail_self_recursion() {
    // self-call as an argument to another call
    assert!(
        recursion_warnings("(defn fact (n) (if (= n 0) 1 (* n (fact (- n 1)))))")
            .iter()
            .any(|w| w.contains("fact") && w.contains("non-tail"))
    );
    assert!(recursion_warnings(
        "(defn sum (xs) (if (empty? xs) 0 (+ (first xs) (sum (rest xs)))))"
    )
    .iter()
    .any(|w| w.contains("sum")));
    // self-call as a let binding value
    assert!(!recursion_warnings("(defn k (n) (let (m (k (- n 1))) m))").is_empty());
    // first (tested) operand of `and`, and a `cond` test
    assert!(!recursion_warnings("(defn p (n) (and (p n) (> n 0)))").is_empty());
    assert!(!recursion_warnings("(defn g (n) (cond (g 0) :a else :b))").is_empty());
}

#[test]
fn no_warning_for_tail_recursion_or_higher_order() {
    // proper tail calls in each tail-propagating special form
    assert!(
        recursion_warnings("(defn go (n acc) (if (= n 0) acc (go (- n 1) (* acc n))))").is_empty()
    );
    assert!(recursion_warnings("(defn down (n) (when (> n 0) (down (- n 1))))").is_empty());
    assert!(recursion_warnings("(defn f (n) (cond (= n 0) :z else (f (- n 1))))").is_empty());
    assert!(recursion_warnings("(defn p (n) (and (> n 0) (p (- n 1))))").is_empty());
    assert!(recursion_warnings("(defn k (n) (let (m (- n 1)) (k m)))").is_empty());
    // a self-call inside a nested closure is a different frame — not flagged
    assert!(recursion_warnings("(defn h (xs) (map xs (fn (x) (h x))))").is_empty());
    // non-recursive function
    assert!(recursion_warnings("(defn g (x) (+ x 1))").is_empty());
}

#[test]
fn unused_let_binding_lint() {
    // Basic unused binding — warned.
    let w = file_warnings("(let (x 1) 2)");
    assert!(
        w.iter()
            .any(|s| s.contains("unused let binding") && s.contains('x')),
        "expected unused-binding warning for x, got {w:?}"
    );
    // Binding used in body — silent.
    assert!(
        file_warnings("(let (x 1) x)").is_empty(),
        "used binding should be silent"
    );
    // Binding used in subsequent binding RHS — silent.
    assert!(
        file_warnings("(let (x 1 y (+ x 1)) y)").is_empty(),
        "x used by y's RHS should be silent"
    );
    // Only one of two is unused.
    let w = file_warnings("(let (x 1 y 2) x)");
    assert!(
        w.iter()
            .any(|s| s.contains("unused let binding") && s.contains('y')),
        "y should be flagged unused, got {w:?}"
    );
    assert!(
        w.iter().all(|s| !s.contains('x') || !s.contains("unused")),
        "x should not be flagged, got {w:?}"
    );
    // `_`-prefixed names are exempt.
    assert!(
        file_warnings("(let (_x 1) 2)").is_empty(),
        "_x should be exempt from unused-binding lint"
    );
    // Gensym temporaries (`<prefix>__<n>`) are exempt: a macro expansion can
    // attach its call-site position to the generated `let`, so the name — not
    // the position — is the reliable "compiler-generated" signal.
    assert!(
        file_warnings("(let (m__1380 1) 2)").is_empty(),
        "gensym-named binding should be exempt from unused-binding lint"
    );
    // …but a hand-written name that merely contains `__` (no trailing digits)
    // is still linted.
    assert!(
        file_warnings("(let (my__thing 1) 2)")
            .iter()
            .any(|s| s.contains("unused let binding")),
        "a non-gensym `__` name should still be flagged"
    );
    // match pattern variables (compiler-generated let, no source position)
    // must be exempt — a common pattern: match on shape, ignore values.
    assert!(
        file_warnings("(match (list 1 2) ([a b] :vec) (_ :other))").is_empty(),
        "match pattern variables should be exempt (no FP)"
    );
    // Nested let: inner binding used only in inner body.
    assert!(
        file_warnings("(let (x 1) (let (y x) y))").is_empty(),
        "nested let: both x and y are used"
    );
    // letrec: mutual recursion keeps both used.
    assert!(
        file_warnings("(letrec (f (fn (n) (if (= n 0) 1 (g (- n 1)))) g (fn (n) (f n))) (f 5))")
            .is_empty(),
        "letrec mutual recursion: both f and g are used"
    );
    // Binding used only inside a map literal — silent. Map literals are
    // heap maps, not pairs, so the occurrence scan must descend into their
    // keys and values too (regression: the editor's `{:start s :end e}`
    // edit forms were all falsely flagged unused).
    assert!(
        file_warnings("(let (s 1) {:start s})").is_empty(),
        "binding used as a map value should be silent"
    );
    assert!(
        file_warnings("(let (k :a) {k 1})").is_empty(),
        "binding used as a map key should be silent"
    );
    // …and a binding used only inside a closure that is itself a map value
    // (the minibuffer `:on-complete (fn …)` pattern).
    assert!(
        file_warnings("(let (p 1) {:on-complete (fn (x) (+ x p))})").is_empty(),
        "binding captured by a closure inside a map should be silent"
    );
    // The map descent must not mask genuine dead bindings.
    let w = file_warnings("(let (s 1) {:start 2})");
    assert!(
        w.iter()
            .any(|s| s.contains("unused let binding") && s.contains('s')),
        "s unused even though a map literal is present, got {w:?}"
    );
}

#[test]
fn curated_equality_and_string_sigs() {
    // = / not= are multi-arm closures; pin bool result so numeric sinks catch it.
    assert!(warnings("(+ 1 (= x y))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    assert!(warnings("(+ 1 (not= x y))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    // string/->symbol requires a string.
    assert!(warnings("(string/->symbol 99)")
        .iter()
        .any(|w| w.contains("string/->symbol") && w.contains("string")));
    // String predicates require string args.
    for f in ["string/starts-with?", "string/ends-with?"] {
        assert!(
            warnings(&format!("({f} 5 \"x\")"))
                .iter()
                .any(|w| w.contains(f) && w.contains("string")),
            "{f}: expected string-domain warning"
        );
    }
    assert!(warnings("(string/blank? 0)")
        .iter()
        .any(|w| w.contains("string/blank?") && w.contains("string")));
    // String transforms require string args and return strings.
    for f in ["string/trim", "string/triml", "string/trimr"] {
        assert!(
            warnings(&format!("({f} 5)"))
                .iter()
                .any(|w| w.contains(f) && w.contains("string")),
            "{f}: expected string-domain warning"
        );
        // Result is string — safe to pass to string-length.
        assert!(
            warnings(&format!("(string/length ({f} s))")).is_empty(),
            "{f}: result should type as string"
        );
    }
    assert!(warnings("(string/replace 5 \"a\" \"b\")")
        .iter()
        .any(|w| w.contains("string/replace") && w.contains("string")));
    assert!(warnings("(string/repeat 3 5)")
        .iter()
        .any(|w| w.contains("string/repeat") && w.contains("string")));
    assert!(warnings("(string/format 5 \"extra\")")
        .iter()
        .any(|w| w.contains("format") && w.contains("string")));
    // format returns a string.
    assert!(warnings("(string/length (string/format \"hi %s\" x))").is_empty());
    // index-of/index-where/string/last-index-of return int — safe to add.
    // (`last-index-of` moved into the `string` module on 2026-08-27; the curated
    // entry is keyed qualified, so the bare name is now correctly unbound.)
    assert!(warnings("(+ 1 (index-of coll x))").is_empty());
    assert!(warnings("(+ 1 (string/last-index-of s needle))").is_empty());
    // Correct uses stay silent.
    for ok in [
        "(= 1 2)",
        "(not= x y)",
        "(string/starts-with? s \"pre\")",
        "(string/ends-with? s \".blsp\")",
        "(string/trim s)",
        "(string/replace s \"a\" \"b\")",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn skips_error_testing_forms() {
    // `try` and the error-asserting helpers deliberately exercise failures,
    // so misuse inside them is not flagged.
    assert!(warnings("(try (first 5) (catch e e))").is_empty());
    assert!(warnings("(error-of (first 5))").is_empty());
    assert!(warnings("(assert-error (first 5))").is_empty());
    // ...but a sibling form outside the skipped one is still checked.
    assert!(!warnings("(do (first 5) (try (first 6) (catch e e)))").is_empty());
}

/// KI-67. An error-testing form suppresses *misuse* — that is what it is for —
/// but an **unbound symbol** inside one is a dead call site, not the failure
/// under test. Skipping the body outright let a rename wave ship a broken `try`
/// with every gate green: hive's spool write was
/// `(try (bytes/append path piece) (catch e …))`, the callee was renamed to
/// `file/spit-bytes-append`, `nest check` said nothing, and every upload broke.
#[test]
fn unbound_inside_an_error_testing_form_is_still_flagged() {
    for src in [
        "(try (definitely-not-bound 1) (catch e e))",
        "(error-of (definitely-not-bound 1))",
        "(assert-error (definitely-not-bound 1))",
        // nested one level down, not just in head position
        "(try (first (definitely-not-bound 1)) (catch e e))",
    ] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("unbound symbol: definitely-not-bound")),
            "{src} should flag the unbound name, got {w:?}"
        );
    }
}

/// KI-71 — a **reversed-args rename** is the one rename mistake with no natural gate: the
/// arity is unchanged and no name is unbound, so `nest check` is silent and the wrong answer
/// surfaces somewhere else entirely (`seq/remove-nth` moving its index read as seven
/// unrelated buffer-lifecycle failures downstream). A declared `sig` is what makes it
/// visible, and the index/collection functions in `std/seq.blsp` now carry one.
///
/// Argument types are precise on purpose and the return is `any`: the reversal is an
/// ARGUMENT mistake, and a too-narrow return would false-positive at every call site.
#[test]
fn a_reversed_index_and_collection_call_is_flagged() {
    // Data-first (ADR-308): the COLLECTION comes first, so an index-first call is the
    // reversal. This pair flipped with the convention — the shapes below used to be the
    // correct ones.
    for (src, fname) in [
        ("(seq/remove-nth 1 [1 2 3])", "remove-nth"),
        ("(seq/take-last 2 (list 1 2 3))", "take-last"),
        ("(seq/chunk-every 2 [1 2 3 4])", "chunk-every"),
        ("(seq/split-at 1 [1 2 3])", "split-at"),
    ] {
        let w = warnings_with(&["seq"], src);
        assert!(
            w.iter()
                .any(|m| m.contains(fname) && m.contains("argument 1 expects seqable")),
            "{src} reverses index and collection and should be flagged, got {w:?}"
        );
    }
}

/// The false-positive half: the CORRECT (collection-first) order must stay silent, and so
/// must a call whose
/// arguments are untyped locals — the checker only knows a param's type when something
/// says so, and guessing would make these sigs unusable.
#[test]
fn the_correct_collection_first_order_stays_silent() {
    for src in [
        "(seq/remove-nth [1 2 3] 1)",
        "(seq/take-last (list 1 2 3) 2)",
        "(seq/chunk-every [1 2 3 4] 2)",
        "(seq/split-at [1 2 3] 1)",
        "(fn (coll i) (seq/remove-nth coll i))",
    ] {
        assert!(
            warnings_with(&["seq"], src).is_empty(),
            "{src} is correct and should be silent, got {:?}",
            warnings_with(&["seq"], src)
        );
    }
}

/// KI-70 — the walk used to `return` for any form that was not a `Pair`, so every
/// expression nested inside a vector or map LITERAL was invisible to every lint.
/// Hiccup-shaped code is written entirely that way, which is how `(str (max 2 …))`
/// survived in hive's `/docs` renderer long after `max` moved to `math`, with
/// `nest check` green and only a page render raising it.
#[test]
fn unbound_inside_a_vector_or_map_literal_is_flagged() {
    for src in [
        "[:tag (definitely-not-bound 1)]",            // vector literal
        "{:k (definitely-not-bound 1)}",              // map literal
        "{(definitely-not-bound 1) :v}",              // map literal, KEY position
        "[:tag {:k (definitely-not-bound 1)}]",       // map inside a vector
        "[:tag {:k (str (definitely-not-bound 1))}]", // the shape found in the wild
        "[[[(definitely-not-bound 1)]]]",             // nested vectors
    ] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("unbound symbol: definitely-not-bound")),
            "{src} should flag the unbound name, got {w:?}"
        );
    }
}

/// The false-positive half of KI-70. Descending into literals must not start
/// reading DATA as code: `quote`/`quasiquote` stop the walk before their contents
/// are ever handed down, and the checker runs on macroexpanded forms, so a `match`
/// pattern vector has already become `let`/`if` binders by the time we get here.
#[test]
fn descending_into_a_literal_does_not_read_data_as_code() {
    for src in [
        "'[a b c]",                                   // quoted vector of bare symbols
        "'{:k v}",                                    // quoted map
        "(quote [definitely-not-bound])",             // explicit quote
        "(match [1 2] ([a b] (+ a b)) (_ 0))",        // pattern binders, post-expansion
        "(let (xs [1 2 3]) (map xs (fn (n) [n n])))", // ordinary literal use
    ] {
        assert!(
            warnings(src).is_empty(),
            "{src} should stay silent, got {:?}",
            warnings(src)
        );
    }
}

/// The other half of KI-67: everything that is *not* an unbound symbol stays
/// suppressed inside an error-testing form. Filtering happens at the collection
/// point, so a lint added later is suppressed here by default — which is the
/// right default for a form whose purpose is to exercise a failure.
#[test]
fn only_unbound_survives_an_error_testing_form() {
    for src in [
        "(error-of (cons 1))",              // arity
        "(try (first 5) (catch e e))",      // type misuse
        "(assert-error (string/length 5))", // sig mismatch
    ] {
        assert!(
            warnings(src).is_empty(),
            "{src} should stay silent, got {:?}",
            warnings(src)
        );
    }
}

/// A test that really does assert on an unbound name opts out explicitly.
#[test]
fn check_allow_unbound_still_silences_an_error_testing_body() {
    assert!(
        warnings("(check-allow :unbound (try (definitely-not-bound 1) (catch e e)))").is_empty()
    );
}

#[test]
fn covers_the_other_signed_primitives() {
    assert!(warnings("(math/mod 7 3)").is_empty());
    assert!(warnings("(math/mod 7 \"x\")")
        .iter()
        .any(|w| w.contains("mod")));
    assert!(warnings("(math/rem :a 3)")
        .iter()
        .any(|w| w.contains("rem")));
    assert!(warnings("(%vector-length 5)")
        .iter()
        .any(|w| w.contains("vector-length")));
    assert!(warnings("(string/substring \"hi\" \"a\" 1)")
        .iter()
        .any(|w| w.contains("string/substring") && w.contains("argument 2")));
    assert!(warnings("(%lt 1 :k)").iter().any(|w| w.contains("%lt")));
}

#[test]
fn reports_each_bad_argument() {
    // Both args provably wrong → two distinct warnings (one per position).
    let w = warnings("(math/mod \"a\" :b)");
    assert_eq!(w.len(), 2, "{:?}", w);
    assert!(w.iter().any(|s| s.contains("argument 1")));
    assert!(w.iter().any(|s| s.contains("argument 2")));
}

#[test]
fn nested_misuse_is_found() {
    // A wrong call buried inside an argument is still reported.
    let w = warnings("(%vector-length (cons (first 5) 2))");
    assert!(w.iter().any(|s| s.contains("first")));
}

#[test]
fn atoms_and_malformed_forms_do_not_panic() {
    for src in [
        "5",
        "foo",
        "\"s\"",
        ":k",
        "()",
        "(5 6 7)",
        "(first)",
        // a bare `(fn)` — no params, no body — panicked the recursion analyzer's
        // body slice (`&items[2..]` on length 1), live in bedit 2026-08-31; the
        // letrec shape is the exact path that crashed an unguarded worker thread
        "(fn)",
        "(letrec (go (fn)) (go))",
        "(let (f (fn)) (f))",
        "(def g (fn))",
        "(defn h () (letrec (go (fn)) go))",
    ] {
        // No panic, and no spurious warning on a bare atom / non-symbol head /
        // missing argument.
        let _ = warnings(src);
    }
    assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol — no diagnostics
                                             // `(first)` is now an arity diagnostic (0 args; first needs 1).
    assert!(warnings("(first)")
        .iter()
        .any(|w| w.contains("first") && w.contains("expected 1")));
}

#[test]
fn combinator_collection_slot_rejects_the_old_argument_order() {
    // The shape of bedit's seven 0.20-migration bugs: a callback or count in the
    // collection slot (pre-ADR-308 order), which sailed through the checker and
    // failed only at runtime. Two rules catch it now: the curated data-first
    // domains (take/drop/mapcat/…), and the fn-LITERAL rule — a lambda whose
    // result can't be inferred has no arrow type, but its tag is never in doubt,
    // so a parameter with no room for a function rejects it regardless.
    for (src, frag) in [
        ("(map (fn (x) x) [1])", "got a function"),
        ("(mapcat (fn (x) x) [1])", "got a function"),
        ("(any? (fn (x) x) [1])", "got a function"),
        ("(take 5 [1 2])", "argument 1 expects seqable"),
        ("(drop 3 [1 2])", "argument 1 expects seqable"),
    ] {
        let ws = warnings(src);
        assert!(ws.iter().any(|w| w.contains(frag)), "{src}: {ws:?}");
    }
    // …and the data-first order stays silent.
    for src in [
        "(map [1] (fn (x) x))",
        "(take [1 2] 5)",
        "(drop [1 2] 3)",
        "(mapcat [1] (fn (x) (list x)))",
        "(fold [1] 0 (fn (a x) a))",
    ] {
        assert!(warnings(src).is_empty(), "{src}: {:?}", warnings(src));
    }
}

// ---- one file, one name, two definitions (Pass 2.9) ----
// `project.blsp` carried a public sorted `source-files` and, 1,300 lines later, a private
// unsorted one; every caller ran the second while the first's docstring made the promise.
// The cross-file lint is per-namespace across files; this is the same-file half.

/// The file-mode warnings for `src`, filtered to the duplicate-definition lint.
fn duplicate_warnings(src: &str) -> Vec<String> {
    file_warnings(src)
        .into_iter()
        .filter(|w| w.contains("is defined twice in this file"))
        .collect()
}

#[test]
fn a_second_top_level_definition_of_a_name_is_flagged_once_naming_both() {
    let w = duplicate_warnings(
        "(defmodule dup)\n(defn helper (x) x)\n(defn other () 1)\n(defn- helper (x) (+ x 1))",
    );
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].contains("`helper`") && w[0].contains("line 2") && w[0].contains("dead code"),
        "{w:?}"
    );
    // Every definer counts, not just `defn`: a `def` shadowed by a `defdyn` is the same bug.
    let w = duplicate_warnings("(defmodule dup)\n(def *x* 1)\n(defdyn *x* 2)");
    assert_eq!(w.len(), 1, "{w:?}");
}

#[test]
fn the_duplicate_lint_stays_silent_where_a_second_binding_is_legitimate() {
    // Distinct names, and a name rebound inside a body (not top level).
    assert!(
        duplicate_warnings("(defmodule dup)\n(defn a () 1)\n(defn b () (def c 1) (def c 2))")
            .is_empty()
    );
    // A second module in the same file (ADR-223 regions) starts a fresh name set.
    assert!(duplicate_warnings(
        "(defmodule one)\n(defn helper () 1)\n(defmodule two)\n(defn helper () 2)"
    )
    .is_empty());
    // A deliberate override says so.
    assert!(duplicate_warnings(
        "(defmodule dup)\n(def *x* :first)\n(check-allow :duplicate-def (def *x* :second))"
    )
    .is_empty());
    // …and a `check-allow` for a DIFFERENT category does not silence it.
    assert_eq!(
        duplicate_warnings(
            "(defmodule dup)\n(def *x* :first)\n(check-allow :unbound (def *x* :second))"
        )
        .len(),
        1
    );
}
