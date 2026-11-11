//! Every finding carries a [`Lint`] category, and a `(defmodule … (:check-allow …))`
//! header clause drops the categories it names for its module (ADR-673 §5).

use super::*;

/// Whole-file findings, categories included — the shape `check_file_findings` returns.
fn file_findings(src: &str) -> Vec<Finding> {
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms: Vec<Value> = crate::syntax::reader::read_all_positioned(&mut heap, src)
        .expect("parse")
        .into_iter()
        .map(|(form, _)| form)
        .collect();
    check_forms(&mut heap, &forms, &[], false, false)
}

fn has(findings: &[Finding], lint: Lint, text: &str) -> bool {
    findings
        .iter()
        .any(|finding| finding.lint == lint && finding.message.contains(text))
}

const NON_TAIL: &str = "(defn fact (n) (if (= n 0) 1 (* n (fact (- n 1)))))";
const MISMATCH: &str = "(defn adds () (+ 1 \"x\"))";

#[test]
fn findings_carry_their_category() {
    let findings = file_findings(&format!(
        "(defmodule category-kinds)\n{NON_TAIL}\n{MISMATCH}\n(defn calls () (no-such-function-here 1))"
    ));
    assert!(
        has(
            &findings,
            Lint::NonTailRecursion,
            "fact: recursive call in non-tail position"
        ),
        "{findings:?}"
    );
    assert!(
        has(&findings, Lint::TypeMismatch, "argument 2 expects"),
        "{findings:?}"
    );
    assert!(
        has(
            &findings,
            Lint::Unbound,
            "unbound symbol: no-such-function-here"
        ),
        "{findings:?}"
    );
}

#[test]
fn header_check_allow_drops_only_the_named_category() {
    let findings = file_findings(&format!(
        "(defmodule category-allowed (:check-allow :non-tail-recursion))\n{NON_TAIL}\n{MISMATCH}"
    ));
    assert!(
        !findings
            .iter()
            .any(|finding| finding.lint == Lint::NonTailRecursion),
        "{findings:?}"
    );
    assert!(
        has(&findings, Lint::TypeMismatch, "argument 2 expects"),
        "the other categories stay: {findings:?}"
    );
}

#[test]
fn header_check_allow_governs_its_own_module_region() {
    // Two modules in one file (ADR-223): the first module's clause does not reach the
    // second module's definitions.
    let findings = file_findings(
        "(defmodule region-one (:check-allow :non-tail-recursion))\n\
         (defn fact (n) (if (= n 0) 1 (* n (fact (- n 1)))))\n\
         (defmodule region-two)\n\
         (defn total (n) (if (= n 0) 0 (+ n (total (- n 1)))))",
    );
    assert!(
        !has(&findings, Lint::NonTailRecursion, "fact:"),
        "{findings:?}"
    );
    assert!(
        has(&findings, Lint::NonTailRecursion, "total:"),
        "{findings:?}"
    );
}

#[test]
fn unknown_header_category_is_a_check_allow_finding() {
    let findings = file_findings(&format!(
        "(defmodule category-typo (:check-allow :no-such-category :non-tail-recursion))\n{NON_TAIL}"
    ));
    assert!(
        has(&findings, Lint::CheckAllow, ":no-such-category"),
        "{findings:?}"
    );
    // The known name beside it still applies.
    assert!(
        !findings
            .iter()
            .any(|finding| finding.lint == Lint::NonTailRecursion),
        "{findings:?}"
    );
}

#[test]
fn every_category_name_round_trips() {
    let mut seen = std::collections::HashSet::new();
    for &lint in Lint::ALL {
        assert_eq!(Lint::from_name(lint.name()), Some(lint), "{lint:?}");
        assert!(seen.insert(lint.name()), "duplicate name {}", lint.name());
    }
    assert_eq!(Lint::from_name("no-such-category"), None);
    // Named by Brood code (the gate in project-check.blsp, the whole-project lints).
    for name in [
        "non-tail-recursion",
        "gave-up",
        "deprecated",
        "beta",
        "unbound",
        "internal",
        "duplicate-def",
        "unused-private",
        "check-allow",
    ] {
        assert!(Lint::from_name(name).is_some(), "{name}");
    }
}

#[test]
fn the_runtime_defmodule_accepts_the_clause() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defmodule category-runtime (:check-allow :unbound :non-tail-recursion))")
        .expect("a :check-allow header clause loads");
}
