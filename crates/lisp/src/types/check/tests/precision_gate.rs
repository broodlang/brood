//! Precision survives macro expansion: every prelude macro that carries a value or a test
//! has a case proving the checker still sees through it.
//!
//! The checker types the EXPANDED program — the eight special forms plus primitives — so a
//! surface macro is only as precise as the shape it lowers to. That is sound by
//! construction (an `(if g X g)` *is* an `and`, whoever wrote it), but it is fragile in one
//! direction: change a macro's expansion and the narrowing through it can vanish with no
//! test going red, because losing precision only ever makes the checker quieter. `condp`
//! was exactly that: it bound its predicate to a temporary, so `(condp x = "a" …)` never
//! narrowed `x` while `case` did. This gate is the precision twin of the reach gate
//! (`reach_gate.rs`): a case per macro whose loss would be silent, and a completeness test
//! that makes a NEW prelude macro say which kind it is.

use super::*;

/// What a case must show.
enum Want {
    /// The checker reports a finding containing this text — which it can only do if the
    /// type flowed through the macro's expansion (an unknown type is never reported).
    Reports(&'static str),
    /// A known precision gap, recorded so it cannot be mistaken for coverage: the case is
    /// asserted SILENT today. When the gap is fixed this assertion fails — move the case to
    /// `Reports` then.
    KnownGap(&'static str),
}

use Want::{KnownGap, Reports};

const NUMBER_GOT_STRING: &str = "+: argument 1 expects number, got string";

/// `(macro, source, what the check must show)`. Each source misuses a value whose type
/// only the macro's expansion can carry: a test it narrows on, a binding it makes, or a
/// value it passes through.
const PRECISION_CASES: &[(&str, &str, Want)] = &[
    // Tests that narrow their branch.
    (
        "and",
        "(defn f (x) (and (string? x) (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "or",
        "(defn f (x) (or (not (string? x)) (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "when",
        "(defn f (x) (when (string? x) (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "cond",
        "(defn f (x) (cond (string? x) (+ x 1) else 0))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "condp",
        "(defn f (x) (condp x = \"a\" (+ x 1) 0))",
        Reports("+: argument 1 expects number, got \"a\""),
    ),
    (
        "case",
        "(defn f (x) (case x \"a\" (+ x 1) 0))",
        Reports("+: argument 1 expects number, got \"a\""),
    ),
    (
        "match",
        "(defn f (x) (match x ((and s (str \"a\" _)) (+ s 1)) (_ 0)))",
        Reports("+: argument 1 expects number, got string"),
    ),
    (
        "cond->",
        "(defn f (x) (cond-> x (string? x) (+ 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    // Bindings that carry a narrowed or produced value.
    (
        "if-let",
        "(defn f (x) (if-let (s (when (string? x) x)) (+ s 1) 0))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "when-let",
        "(defn f (x) (when-let (s (when (string? x) x)) (+ s 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "if-some",
        "(defn f (x) (if-some (s (when (string? x) x)) (+ s 1) 0))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "when-some",
        "(defn f (x) (when-some (s (when (string? x) x)) (+ s 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "with",
        "(defn f (x) (with (n (string/->number x)) (string/upper n)))",
        Reports("string/upper: argument 1 expects string, got number"),
    ),
    (
        "ok->",
        "(defn f (x) (ok-> x string/->number string/upper))",
        Reports("string/upper: argument 1 expects string, got number"),
    ),
    (
        "try",
        "(defn g () 1) (defn f () (try (g) (catch (and e {:kind :type}) (+ e 1))))",
        Reports("+: argument 1 expects number, got map | record"),
    ),
    (
        "receive",
        "(defn f () (receive (s :when (string? s) (+ s 1))))",
        KnownGap(
            "a clause's bindings reach its body through `(nth rcv i)` of `%receive`'s result, \
             which the checker types as unknown",
        ),
    ),
    // Comprehension and clause guards.
    (
        "for",
        "(defn f (xs) (for (x xs :when (string? x)) (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "fold-for",
        "(defn f (xs) (fold-for (acc 0 x xs :when (string? x)) (+ x acc)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "doseq",
        "(defn f (xs) (doseq (x xs :when (string? x)) (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "defn",
        "(defn f ((x) :when (string? x) (+ x 1)) ((x) 0))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "defn-",
        "(defn- f ((x) :when (string? x) (+ x 1)) ((x) 0))",
        Reports(NUMBER_GOT_STRING),
    ),
    // Values that pass through.
    (
        "->",
        "(defn f (x:string) (-> x (+ 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "as->",
        "(defn f (x:string) (as-> x v (+ v 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "doto",
        "(defn f (x:string) (doto x (+ 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "debug/spy",
        "(defn f (x:string) (+ (debug/spy x) 1))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "assert",
        "(defn f (x:string) (assert (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "binding",
        "(defn f (x:string) (binding (*out* *err*) (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "check-allow",
        "(defn f (x:string) (check-allow :unused-binding (+ x 1)))",
        Reports(NUMBER_GOT_STRING),
    ),
    (
        "spawn",
        "(defn f () (+ (spawn (fn () nil)) 1))",
        Reports("got pid"),
    ),
    (
        "spawn-link",
        "(defn f () (+ (spawn-link (fn () nil)) 1))",
        Reports("got pid"),
    ),
    (
        "spawn-monitor",
        "(defn f () (+ (spawn-monitor (fn () nil)) 1))",
        Reports("got vector"),
    ),
    (
        "with-out-str",
        "(defn f (x:string) (+ (with-out-str (io/puts x)) 1))",
        KnownGap("the captured string is not typed: the result reads as unknown"),
    ),
    (
        "with-err-str",
        "(defn f (x:string) (+ (with-err-str (io/puts x)) 1))",
        KnownGap("the captured string is not typed: the result reads as unknown"),
    ),
];

/// Prelude macros with no precision case, each with the reason none applies.
const NO_PRECISION_CASE: &[(&str, &str)] = &[
    (
        "%match",
        "the engine under match, case and receive — covered through them",
    ),
    (
        "comment",
        "its body never runs; the reach gate pins that it stays unread",
    ),
    (
        "def-",
        "a definer: the value it binds is checked as a def's",
    ),
    (
        "defability",
        "a definer of dispatch, checked by ability conformance",
    ),
    ("defdelegate", "a definer that forwards a call"),
    ("defdyn", "a definer of a dynamic name"),
    ("defmethod", "a definer of a multimethod arm"),
    ("defmodule", "a module header"),
    ("defmulti", "a definer of dispatch"),
    (
        "defonce",
        "a definer: the value it binds is checked as a def's",
    ),
    (
        "defrecord",
        "a definer whose constructor sig the checker reads directly",
    ),
    ("%defseq", "a definer of std's sequence functions"),
    ("deftype", "a declaration: no value at run time"),
    (
        "impl",
        "registers an ability's methods, checked by ability conformance",
    ),
    ("include-bytes", "reads a file at expansion time"),
    ("include-str", "reads a file at expansion time"),
    ("meta", "attaches metadata to a definition"),
    ("sig", "a declaration: no value at run time"),
    ("sig!", "a declaration: no value at run time"),
    (
        "%swap-registry!",
        "a registry write in the prelude's own bookkeeping",
    ),
    ("%unimpl", "retracts an impl"),
];

#[test]
fn precision_survives_each_macros_expansion() {
    for (name, src, want) in PRECISION_CASES {
        let warnings = file_warnings(src);
        match want {
            Reports(text) => assert!(
                warnings.iter().any(|w| w.contains(text)),
                "`{name}`: the type no longer flows through its expansion — expected a \
                 finding containing `{text}` for `{src}`, got {warnings:?}"
            ),
            KnownGap(reason) => assert!(
                warnings.iter().all(|w| !w.contains("expects")),
                "`{name}`: a recorded precision gap ({reason}) now reports — the gap is \
                 closed, so move this case to `Reports`: `{src}` gave {warnings:?}"
            ),
        }
    }
}

/// The prelude's macro names, read from its source: `(defmacro NAME` at a line's start.
fn prelude_macro_names() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std/prelude");
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("std/prelude") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("blsp") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("(defmacro ") {
                let name: String = rest
                    .chars()
                    .take_while(|ch| !ch.is_whitespace() && *ch != '(' && *ch != ')')
                    .collect();
                names.push(name);
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_prelude_macro_is_classified_by_the_precision_gate() {
    // The completeness half: a macro added to the prelude with no entry here would inherit
    // whatever precision its expansion happens to keep, unwatched. Adding one now fails
    // this test until someone writes its case, or says why none applies.
    let names = prelude_macro_names();
    assert!(
        names.len() > 40,
        "the prelude read found too few macros: {names:?}"
    );
    for name in &names {
        let has_case = PRECISION_CASES.iter().any(|(case, _, _)| case == name);
        let exempt = NO_PRECISION_CASE.iter().any(|(case, _)| case == name);
        assert!(
            has_case || exempt,
            "prelude macro `{name}` has no precision-gate case and no recorded reason"
        );
        assert!(!(has_case && exempt), "`{name}` is both covered and exempt");
    }
    // …and no entry outlives its macro.
    for case in PRECISION_CASES
        .iter()
        .map(|(case, _, _)| *case)
        .chain(NO_PRECISION_CASE.iter().map(|(case, _)| *case))
    {
        assert!(
            names.iter().any(|name| name == case),
            "the precision gate names `{case}`, which the prelude no longer defines"
        );
    }
}
