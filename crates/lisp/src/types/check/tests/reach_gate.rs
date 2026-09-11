//! The walk is TOTAL: every code position and every special form is reached (KI-67/KI-70).

use super::*;

// ---- the walk is TOTAL (the reach gate) ----
// KI-67 (`try` bodies) and KI-70 (vector/map literals) were the same bug at two
// depths: a `return`-early line in the walk behind which no lint ran at all, and
// which left no trace to grep for — a lint that is never *reached* is invisible in
// a way a suppressed one is not. Both were found by accident. This is the gate that
// makes a third one fail a test instead: every recognised special form, and every
// container literal, gets a planted unresolvable name in each of its *code*
// positions, and must report it. The two data-holding forms must stay silent.

/// `(head, source with a planted `zzz-…` name, must the walk report it?)`.
/// Every entry in `SPECIAL_HEAD` must appear here — see
/// `every_special_form_is_covered_by_the_reach_gate`.
const REACH_CASES: &[(&str, &str, bool)] = &[
    // Data, not code — reporting here would flag a sketched or quoted name.
    ("quote", "(defn f () (quote (zzz-q)))", false),
    ("comment", "(defn f () (comment (zzz-c)))", false),
    // A template is data, but its `~` escapes are code evaluated at expansion time.
    ("quasiquote", "(defmacro m (x) `(a ~(zzz-qq x)))", true),
    ("quasiquote", "(defmacro m (x) `(a zzz-quoted ~x))", false),
    // Deliberate-failure forms: every other lint is suppressed inside them, but an
    // unbound name is never the failure under test (KI-67).
    ("try", "(defn f () (try (zzz-try) (catch e e)))", true),
    (
        "%try",
        "(defn f () (%try (fn () (zzz-tp)) (fn (e) e)))",
        true,
    ),
    ("error-of", "(defn f () (error-of (zzz-eo)))", true),
    ("assert-error", "(defn f () (assert-error (zzz-ae)))", true),
    // Ordinary code positions.
    ("if", "(defn f (b) (if (zzz-test b) 1 2))", true),
    ("if", "(defn f (b) (if b (zzz-then) 2))", true),
    ("if", "(defn f (b) (if b 1 (zzz-else)))", true),
    ("let", "(defn f () (let (a (zzz-rhs)) a))", true),
    ("let", "(defn f () (let (a 1) (zzz-body a)))", true),
    (
        "letrec",
        "(defn f () (letrec (g (fn () (zzz-lr))) (g)))",
        true,
    ),
    ("fn", "(defn f () (fn () (zzz-fn)))", true),
    ("def", "(def x (zzz-def))", true),
    ("defn", "(defn f () (zzz-defn))", true),
    ("defmacro", "(defmacro m (x) (zzz-dm x))", true),
];

/// Container literals — KI-70's class, kept beside the special forms because it is
/// the same question ("does the walk go in?") for the other half of the syntax.
const REACH_CONTAINER_CASES: &[(&str, bool)] = &[
    ("(defn f (x) [:tag (zzz-vec x)])", true),
    ("(defn f (x) {:k (zzz-mapval x)})", true),
    ("(defn f (x) {(zzz-mapkey x) :v})", true),
    ("(defn f (x) [:tag {:k (str (zzz-deep x))}])", true), // the hive shape (KI-70)
    ("(defn f (x) (list [1 2] {:a 1}))", false),           // ordinary literals: silent
];

#[test]
fn the_walk_reaches_every_code_position() {
    for (head, src, must_report) in REACH_CASES {
        let planted = planted_name(src);
        let want = format!("unbound symbol: {planted}");
        // Both walks, because they are different code paths reaching the same arm:
        // `check_file` (what `nest check` runs, with whole-file facts) and
        // `check_form` on the expanded fragment (what the REPL, the LSP and the MCP
        // `check` tool run). A form skipped by one and covered by the other is how
        // this gate's own first sabotage attempt passed — file mode caught the
        // quasiquote escape through an unrelated whole-file pass while the arm under
        // test was doing nothing.
        for (mode, ws) in [
            ("file", file_warnings(src)),
            ("fragment", warnings_expanded(src)),
        ] {
            let reported = ws.contains(&want);
            assert_eq!(
                reported, *must_report,
                "`{head}` ({mode}): expected report={must_report} for `{planted}` in `{src}` — got {ws:?}"
            );
        }
    }
    for (src, must_report) in REACH_CONTAINER_CASES {
        let ws = file_warnings(src);
        let reported = ws.iter().any(|w| w.starts_with("unbound symbol: zzz-"));
        assert_eq!(
            reported, *must_report,
            "container reach: `{src}` — got {ws:?}"
        );
    }
}

#[test]
fn every_special_form_is_covered_by_the_reach_gate() {
    // The completeness half: a head added to `SPECIAL_HEAD` with no case here would
    // otherwise inherit whatever reach it happened to get, unwatched — which is
    // exactly how KI-67 and KI-70 survived. Adding a head now fails this test until
    // someone says, in a case above, what the walk is supposed to do with its body.
    for &sym in super::walk::SPECIAL_HEAD.keys() {
        let name = crate::core::value::symbol_name(sym);
        assert!(
            REACH_CASES.iter().any(|(head, _, _)| *head == name),
            "special form `{name}` has no reach-gate case in REACH_CASES"
        );
    }
}
