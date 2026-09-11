//! The `:discarded-catch` lint.

use super::*;

// ---- discarded catch (`:discarded-catch`) ----
// A `(catch e <constant>)` handler cannot have read the error it caught, so it
// swallows an unbound symbol (a rename) or a real fault unseen. The lint reads the
// UN-expanded forms, so only an author-written catch is seen — never the one a
// macro such as `assert-error` builds.

#[test]
fn discarded_catch_warns_on_constant_handler_bodies() {
    for src in [
        "(defn f () (try (gui-font 1) (catch e nil)))",
        "(defn f () (try (gui-font 1) (catch _ nil)))",
        "(defn f () (try (gui-font 1) (catch e false)))",
        "(defn f () (try (gui-font 1) (catch e (do))))",
    ] {
        let w = file_warnings(&format!("(defn gui-font (x) x) {src}"));
        assert!(
            w.iter()
                .any(|m| m.starts_with(discarded_catch::DISCARDED_CATCH_PREFIX)),
            "must warn on a discarded catch ({src}): {w:?}"
        );
    }
}

#[test]
fn discarded_catch_stays_silent_when_the_error_is_used_or_handled() {
    for src in [
        "(defn f () (try (gui-font 1) (catch e (log/warn (error-message e)))))",
        "(defn f () (try (gui-font 1) (catch _ (fallback))))",
        "(defn f () (try (gui-font 1) (catch e (if (map? e) nil e))))",
        "(defn f () (try (gui-font 1)))",
        "(defn f () (try (gui-font 1) (catch e e)))",
        // A SENTINEL is intent — "it threw" as a value — not a swallowed error.
        "(defn f () (try (gui-font 1) (catch e :raised)))",
        "(defn f () (try (gui-font 1) (catch _ true)))",
        "(defn f () (try (gui-font 1) (catch _ \"failed\")))",
        "(defn f () (try (gui-font 1) (catch _ 0)))",
        // Macro-built catches are the macro's business, not the author's.
        "(defmacro swallow (& body) `(try (do ~@body) (catch e nil)))",
        "(defn f () (assert-error (gui-font 1)))",
    ] {
        let w = file_warnings(&format!("(defn gui-font (x) x) (defn fallback () 1) {src}"));
        assert!(
            w.iter()
                .all(|m| !m.starts_with(discarded_catch::DISCARDED_CATCH_PREFIX)),
            "must not warn when the catch is used or handled ({src}): {w:?}"
        );
    }
}

#[test]
fn discarded_catch_check_allow_suppresses_only_its_category() {
    let src = "(defn gui-font (x) x) (defn f () (try (gui-font 1) (catch e nil)))";
    let w = file_warnings(&format!("(check-allow :discarded-catch {src})"));
    assert!(
        w.iter()
            .all(|m| !m.starts_with(discarded_catch::DISCARDED_CATCH_PREFIX)),
        "check-allow :discarded-catch must suppress: {w:?}"
    );
    // The opt-out works on the inner expression too, not only at the top level.
    let w = file_warnings(
        "(defn gui-font (x) x) (defn f () (check-allow :discarded-catch (try (gui-font 1) (catch e nil))))",
    );
    assert!(
        w.iter()
            .all(|m| !m.starts_with(discarded_catch::DISCARDED_CATCH_PREFIX)),
        "inner check-allow :discarded-catch must suppress: {w:?}"
    );
    // A mismatched category does not suppress.
    let w = file_warnings(&format!("(check-allow :unbound {src})"));
    assert!(
        w.iter()
            .any(|m| m.starts_with(discarded_catch::DISCARDED_CATCH_PREFIX)),
        "a mismatched category must not suppress the discarded-catch lint: {w:?}"
    );
}

#[test]
fn zz_probe_failure() {
    let cases = [
        ("arg into a native", "(defmodule t)\n(sig p (string -> (or string failure)))\n(defn p (s) s)\n(defn q (s) (string/length (p s)))"),
        ("arg into a local defn", "(defmodule t)\n(sig p (string -> (or string failure)))\n(defn p (s) s)\n(sig r (string -> int))\n(defn r (s) 1)\n(defn q (s) (r (p s)))"),
        ("declared return", "(defmodule t)\n(sig p (string -> (or string failure)))\n(defn p (s) s)\n(sig q (string -> string))\n(defn q (s) (p s))"),
        ("arithmetic", "(defmodule t)\n(sig p (string -> (or number failure)))\n(defn p (s) 1)\n(defn q (s) (* 2 (p s)))"),
        ("guarded — must stay silent", "(defmodule t)\n(sig p (string -> (or string failure)))\n(defn p (s) s)\n(defn q (s) (let (v (p s)) (if (failure? v) 0 (string/length v))))"),
    ];
    for (name, src) in cases {
        eprintln!(
            "PROBE [{name}] plain   = {:?}",
            file_warnings_mode(src, false)
        );
        eprintln!(
            "PROBE [{name}] strict  = {:?}",
            file_warnings_mode(src, true)
        );
    }
}

/// Where a guard's narrowing reaches, as a matrix — every guard shape crossed with every
/// way of denoting the guarded value, over CORRECT programs that must draw no warning.
///
/// ADR-316 reports a failure nothing guards, so every hole in this matrix is a **false
/// positive**, not merely lost precision — the one class this checker is not allowed to
/// have. It is a table rather than five separate tests because the bugs found on the way
/// here were all of the form "it narrows in shape A and not shape B", and a table is the
/// only arrangement in which that is visible at a glance.
///
/// `KnownGap` rows are the reach boundary, recorded rather than hidden. A gap that starts
/// passing fails this test too — that is deliberate: it means the boundary moved and the
/// note explaining it is now wrong.
#[test]
fn a_guard_narrows_across_every_shape_and_value_form() {
    #[derive(PartialEq)]
    enum Expect {
        Silent,
        /// Reported today, with the reason. Not a defect in the guard machinery.
        KnownGap(&'static str),
    }
    use Expect::*;

    let base = "(defmodule t)\n\
                (sig mine (string -> (or string failure)))\n(defn mine (s) s)\n";
    // How the guarded value is denoted. A LOCAL always works; a repeated CALL works only
    // where the callee is known deterministic (`guards::DETERMINISTIC_UNARY`).
    let values: [(&str, &str, &str, Expect); 3] = [
        ("local", "v", "(let (v (string/->number s))", Silent),
        ("listed-path", "(string/->number s)", "(do", Silent),
        (
            "user-fn-path",
            "(mine s)",
            "(do",
            KnownGap(
                "a user function is not in DETERMINISTIC_UNARY, so the repeated call is not \
                 one path and the guard cannot reach the second occurrence. Binding it in a \
                 `let` works. Closing this needs inferred determinism, not a longer list.",
            ),
        ),
    ];
    let guards = [
        ("if", "(if (failure? {V}) 0 {U})"),
        ("not", "(if (not (failure? {V})) {U} 0)"),
        ("when+error", "(do (when (failure? {V}) (error \"x\")) {U})"),
        ("cond", "(cond (failure? {V}) 0 else {U})"),
        ("nested-if", "(if (failure? {V}) 0 (if true {U} 0))"),
    ];

    let mut wrong = Vec::new();
    for (vname, v, open, expect) in &values {
        for (gname, tmpl) in guards {
            let use_site = if *vname == "user-fn-path" || v.contains("mine") {
                format!("(string/length {v})")
            } else {
                format!("(+ 1 {v})")
            };
            let body = tmpl.replace("{V}", v).replace("{U}", &use_site);
            let src = format!("{base}(defn q (s) {open} {body}))");
            let silent = file_warnings_mode(&src, false).is_empty();
            match (expect, silent) {
                (Silent, false) => wrong.push(format!(
                    "{vname}/{gname}: a correct program was REPORTED — a false positive"
                )),
                (KnownGap(_), true) => wrong.push(format!(
                    "{vname}/{gname}: the recorded gap is closed; delete the KnownGap note"
                )),
                _ => {}
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The narrowing must reach the CALLER, not just the body — the walk and inference are two
/// separate readings and three times in one session a rule landed in only one of them. When
/// that happens the function checks clean and its inferred RETURN still carries the arm the
/// guard removed, so every call site is reported instead of the callee. There is no way to
/// see that from inside the function, which is why this is its own test.
#[test]
fn a_guards_narrowing_reaches_the_caller_through_the_inferred_return() {
    // `f` must RETURN the narrowed value, or the test proves nothing: an `f` that answers
    // `(str v)` is a string whether or not the guard narrowed, which is how the first
    // version of this test passed with the inference half deleted.
    let base = "(defmodule t)\n";
    for (name, def) in [
        (
            "if",
            "(defn f (s) (let (v (string/->number s)) (if (failure? v) 0 v)))",
        ),
        (
            "when+error",
            "(defn f (s) (let (v (string/->number s)) (when (failure? v) (error \"x\")) v))",
        ),
        (
            "listed-path",
            "(defn f (s) (if (failure? (string/->number s)) 0 (string/->number s)))",
        ),
    ] {
        let src = format!("{base}{def}\n(defn caller (s) (+ 1 (f s)))");
        assert!(
            file_warnings_mode(&src, false).is_empty(),
            "[{name}] the callee is silent but its caller is not — the narrowing reached the \
             walk and not the inferred return:\n{src}"
        );
    }
}

/// The narrowing declines wherever the two occurrences are not provably the same value.
/// This is the SOUND direction of `PathKey::Call` — every row here is a case where
/// narrowing would be wrong, and the mechanism has to notice on its own, because nothing
/// downstream would.
///
/// The identity is a canonical key, `(base symbol, [ordered steps])`, computed
/// independently for each occurrence — so two occurrences meet only by normalising to the
/// same key. That is what makes the first row work and every other row decline.
///
/// Each row guards a DIFFERENT mechanism, which is worth knowing before reading a failure:
/// the base half of the key, the step half, `Ctx`'s invalidation on rebinding, and
/// `path_of` requiring a symbol base. None of them guards the allow-list — widening
/// `path_of` to accept any unary call leaves this test green, because these all decline for
/// other reasons. The allow-list's boundary is the `KnownGap` row in
/// `a_guard_narrows_across_every_shape_and_value_form`.
#[test]
fn a_repeated_call_narrows_only_when_it_is_provably_the_same_value() {
    let reported = |src: &str| !file_warnings_mode(src, false).is_empty();

    // Same base, same step: one key, so the guard reaches the second occurrence.
    assert!(!reported(
        "(defmodule t)\n\
         (defn q (expr) (if (failure? (string/->number expr)) 0 (+ 1 (string/->number expr))))"
    ));
    // A DIFFERENT base — `expr` vs `exprs`, which is one keystroke apart and a different
    // value. Different key, no narrowing.
    assert!(reported(
        "(defmodule t)\n\
         (defn q (expr exprs) \
            (if (failure? (string/->number expr)) 0 (+ 1 (string/->number exprs))))"
    ));
    // A different FUNCTION over the same base: also a different key.
    assert!(reported(
        "(defmodule t)\n\
         (defn q (expr) \
            (if (failure? (string/->number expr)) 0 (+ 1 (encoding/hex-decode expr))))"
    ));
    // The base REBOUND between the guard and the use: `Ctx` drops every path keyed on a
    // symbol that is rebound, so the shadowed occurrence is a fresh unknown.
    assert!(reported(
        "(defmodule t)\n\
         (defn q (expr) (if (failure? (string/->number expr)) 0 \
            (let (expr \"zz\") (+ 1 (string/->number expr)))))"
    ));
    // No symbol base at all. `(io/read-line)` cannot be keyed, which is exactly right:
    // it is the case where two evaluations genuinely differ, and the shape that cannot be
    // narrowed is the shape that must not be.
    assert!(reported(
        "(defmodule t)\n\
         (defn q () (if (failure? (string/->number (io/read-line))) 0 \
            (+ 1 (string/->number (io/read-line)))))"
    ));
}

// A `failure` is TRUTHY. `(if (string/->number s) …)` therefore takes the THEN branch
// exactly when the parse FAILED — the opposite of how the shape reads. The checker already
// caught the downstream consequence when the value flowed somewhere typed; a test whose only
// job is the branch, or an untyped consumer, sailed through. A registry shipped
// `(string/->number id)` into a query on the strength of that and answered 500 on any
// non-numeric URL.
#[test]
fn a_failure_used_as_a_condition_is_warned_about() {
    let w = warnings("(defn f (s) (if (string/->number s) :parsed :nope))");
    assert!(
        w.iter().any(|m| m.contains("TRUTHY")),
        "expected a truthy-failure warning, got {w:?}"
    );
}

#[test]
fn the_same_holds_for_or_and_when_which_desugar_to_if() {
    // `warnings` checks ONE form and does not macroexpand, so the desugared shape is
    // asserted here - which is exactly what the checker walks in a real file. Confirmed
    // separately: a file containing `(defn f (s) (or (string/->number s) 0))` warns.
    let or_w = warnings("(defn f (s) (let (t (string/->number s)) (if t t 0)))");
    assert!(
        or_w.iter().any(|m| m.contains("TRUTHY")),
        "`or` yields the FAILURE, not the fallback: {or_w:?}"
    );
    let when_w = warnings("(defn f (s) (let (n (string/->number s)) (if n n nil)))");
    assert!(
        when_w.iter().any(|m| m.contains("TRUTHY")),
        "expected a truthy-failure warning from `when`, got {when_w:?}"
    );
}

// The fix the message names must actually silence it, or the warning is unactionable.
#[test]
fn narrowing_on_the_wanted_type_silences_it() {
    let w = warnings("(defn f (s) (if (int? (string/->number s)) :parsed :nope))");
    assert!(
        !w.iter().any(|m| m.contains("TRUTHY")),
        "narrowing with int? should silence it, got {w:?}"
    );
    let explicit = warnings("(defn f (s) (if (failure? (string/->number s)) :nope :parsed))");
    assert!(
        !explicit.iter().any(|m| m.contains("TRUTHY")),
        "testing failure? explicitly should silence it, got {explicit:?}"
    );
}

/// The lint's own advice has to WORK on a collection, not only on a scalar.
///
/// `(filter xs int?)` keeps exactly the items `int?` admits, so nothing downstream of it can
/// be a failure — but `filter` used to pass the element type straight through, so the lint
/// fired on code that had narrowed exactly as its message tells you to ("narrow with the type
/// you want (`int?` …)"). A diagnostic whose recommended remedy does not silence it is worse
/// than no diagnostic: it teaches the reader to ignore the message.
///
/// Found on bedit, where it reddened CI's downstream gate on correct code:
/// `(or (second (filter (map parts string/->number) int?)) 1)`.
#[test]
fn filtering_a_collection_on_a_type_predicate_narrows_its_elements() {
    // `file_warnings`, not `warnings`: the bare-fragment harness cannot see through
    // `map`/`second` here — the element type comes back unknown, no failure is in the type,
    // and BOTH cases pass vacuously. The control below catches that, and did, twice.
    let narrowed = file_warnings(
        "(defn f (parts) (let (nums (filter (map parts string/->number) int?)) \
         (or (second nums) 1)))",
    );
    assert!(
        !narrowed.iter().any(|m| m.contains("TRUTHY")),
        "filtering on int? should silence it, got {narrowed:?}"
    );
    // The control: WITHOUT the filter a failure genuinely reaches the condition, so the lint
    // must still fire. Otherwise the case above would pass on a build where the lint is dead.
    let unfiltered = file_warnings(
        "(defn f (parts) (let (nums (map parts string/->number)) (or (second nums) 1)))",
    );
    assert!(
        unfiltered.iter().any(|m| m.contains("TRUTHY")),
        "without the filter the failure is real and must warn, got {unfiltered:?}"
    );
}

// ADR-310's rule: a bound known only by EXCLUSION admits failure the way it admits
// everything. Reading that as "can fail" would fire on every unannotated parameter, which
// is most conditions in most programs.
#[test]
fn an_ordinary_untyped_condition_is_not_warned_about() {
    for src in [
        "(defn f (x) (if x :yes :no))",
        "(defn f (x) (if (nil? x) :no :yes))",
        "(defn f (xs) (if (empty? xs) :empty :some))",
        "(defn f (x) (when x x))",
    ] {
        let w = warnings(src);
        assert!(
            !w.iter().any(|m| m.contains("TRUTHY")),
            "{src} should not warn, got {w:?}"
        );
    }
}
