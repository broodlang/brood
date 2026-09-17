//! A file's verdict is a function of the file, not of what the process checked before it
//! (B8, 2026-09-17). The registries are process state — a module's `defmethod`s stay
//! registered after its load — so a check that reads them whole answers differently after
//! another file required that module. The checker reads a registration only when its
//! namespace is in the file's require closure.

fn signatures_in(interp: &crate::Interp, src: &str) -> Vec<(String, String)> {
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    super::file_signatures(&mut heap, &forms)
        .into_iter()
        .map(|s| (s.name, s.sig.to_string()))
        .collect()
}

fn sig_of(sigs: &[(String, String)], name: &str) -> String {
    sigs.iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| panic!("no signature for {name} in {sigs:?}"))
}

const COUNT_UP: &str = "(defmodule order-probe)
(defn count-up (i n acc)
  (cond (>= i n) acc :else (count-up (+ i 1) n (+ acc i))))
(defn use-it () (count-up 0 10 0))";

#[test]
fn another_files_requires_do_not_widen_this_files_operator_domain() {
    // `datetime` registers `compare-to` for its records; a file that never requires it
    // must still read `<`'s domain as plain `number` after some earlier check loaded it.
    let interp = crate::Interp::new();
    let before = sig_of(&signatures_in(&interp, COUNT_UP), "order-probe/count-up");
    let _ = signatures_in(&interp, "(defmodule loads-datetime (:use datetime))");
    let after = sig_of(&signatures_in(&interp, COUNT_UP), "order-probe/count-up");
    assert_eq!(
        before, after,
        "the earlier check leaked into this file's domain"
    );
    assert!(
        before.starts_with("(number, number"),
        "a file with no ordered record in its world compares numbers: {before}"
    );
}

#[test]
fn a_files_own_requires_do_widen_its_operator_domain() {
    // The same probe requiring `datetime` DOES see its `compare-to` records: the filter
    // is by visibility, not a blanket exclusion of the registry.
    let interp = crate::Interp::new();
    let src = COUNT_UP.replace(
        "(defmodule order-probe)",
        "(defmodule order-probe (:use datetime))",
    );
    let with = sig_of(&signatures_in(&interp, &src), "order-probe/count-up");
    assert!(
        with.starts_with("(ordered, ordered"),
        "a file that reaches `datetime` compares its records too: {with}"
    );
}
