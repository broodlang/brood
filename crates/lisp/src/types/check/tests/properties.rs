//! Declared function properties (ADR-351): `:pure` and `:total` on a `(sig …)`, and the
//! `ui-memo` thunk check that needs no declaration.

use super::*;

fn only(ws: &[String], needle: &str) {
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains(needle), "{ws:?}");
}

// ---- :pure ----

#[test]
fn a_pure_function_that_performs_an_effect_is_reported() {
    let src = "(defmodule t)\n\
         (sig area (number -> number) :pure)\n\
         (defn area (r) (* 3 r r))\n\
         (sig shout (string -> nil) :pure)\n\
         (defn shout (s) (io/puts s))";
    only(
        &file_warnings(src),
        "t/shout is declared :pure but performs an effect: (io/puts …)",
    );
}

#[test]
fn purity_is_checked_through_the_functions_a_body_calls() {
    // The effect is two calls down, in a same-file function.
    let src = "(defmodule t)\n\
         (defn- log-it (x) (io/puts (str x)) x)\n\
         (defn- twice (x) (log-it (* 2 x)))\n\
         (sig quad (int -> int) :pure)\n\
         (defn quad (x) (twice (twice x)))";
    only(
        &file_warnings(src),
        "t/quad is declared :pure but performs an effect: calls t/twice, which performs an \
         effect: calls t/log-it, which performs an effect: (io/puts …)",
    );
    // A declared-pure callee is trusted (it is checked at its own definition), and a
    // recursive pure function is pure.
    let src2 = "(defmodule t)\n\
         (sig sum (int int -> int) :pure)\n\
         (defn sum (n acc) (if (<= n 0) acc (sum (- n 1) (+ acc n))))\n\
         (sig tri (int -> int) :pure)\n\
         (defn tri (n) (sum n 0))";
    assert!(file_warnings(src2).is_empty(), "{:?}", file_warnings(src2));
}

#[test]
fn a_callback_handed_to_a_call_runs_but_a_returned_closure_does_not() {
    let src = "(defmodule t)\n\
         (sig each-loud ((list int) -> any) :pure)\n\
         (defn each-loud (xs) (map xs (fn (x) (io/puts (str x)))))\n\
         (sig make-logger (string -> fn) :pure)\n\
         (defn make-logger (prefix) (fn (x) (io/puts (str prefix x))))";
    only(
        &file_warnings(src),
        "t/each-loud is declared :pure but performs an effect: (io/puts …)",
    );
}

#[test]
fn a_ui_memo_thunk_that_performs_an_effect_is_reported() {
    let src = "(defmodule t)\n\
         (defn render (n) (editor/ui/ui-memo [:line n] n (fn () (io/puts \"painting\") [])))\n\
         (defn quiet (n) (editor/ui/ui-memo [:line n] n (fn () [])))";
    only(
        &file_warnings(src),
        "ui-memo caches a fragment whose thunk performs an effect: (io/puts …)",
    );
}

#[test]
fn a_properties_only_sig_declares_no_type() {
    // `(sig f :pure)` — no type, so no signature is recorded and no call is checked
    // against one; the property still holds the body.
    let src = "(defmodule t)\n\
         (sig f :pure)\n\
         (defn f (x) (table/put x :k 1))\n\
         (defn g () (f 1 2 3))";
    let ws = file_warnings(src);
    assert!(
        ws.iter()
            .any(|w| w.contains("t/f is declared :pure but performs an effect: (table/put …)")),
        "{ws:?}"
    );
    // Whatever else is said (the arity slip in `g`, and `f`'s INFERRED demand for a
    // table, are the ordinary lints'), the property alone declared no signature.
    assert!(
        signatures(src)
            .iter()
            .any(|(name, _, declared)| name == "t/f" && !declared),
        "{:?}",
        signatures(src)
    );
}

// ---- :total ----

#[test]
fn a_total_function_must_decrease_an_argument_on_every_self_call() {
    // `(- n 1)` under `(<= n 0)`'s else: `n ≥ 1` there, bounded below — decreasing.
    let src = "(defmodule t)\n\
         (sig sum (int int -> int) :total)\n\
         (defn sum (n acc) (if (<= n 0) acc (sum (- n 1) (+ acc n))))\n\
         (sig len ((list any) int -> int) :total)\n\
         (defn len (xs acc) (if (nil? xs) acc (len (rest xs) (+ acc 1))))";
    assert!(file_warnings(src).is_empty(), "{:?}", file_warnings(src));
    // `(= n 0)`'s else says only `n ≠ 0`: nothing bounds `n` below, so `(- n 1)` may
    // descend forever — and the checker says so.
    let src2 = "(defmodule t)\n\
         (sig sum (int int -> int) :total)\n\
         (defn sum (n acc) (if (= n 0) acc (sum (- n 1) (+ acc n))))";
    only(
        &file_warnings(src2),
        "t/sum is declared :total but a self-call hands no parameter a structural decrease",
    );
    // A self-call that grows its argument under no upper bound (`n ≥ 0` in the else).
    let src3 = "(defmodule t)\n\
         (sig up (int -> int) :total)\n\
         (defn up (n) (if (< n 0) n (up (+ n 1))))";
    only(
        &file_warnings(src3),
        "t/up is declared :total but a self-call hands no parameter a structural decrease",
    );
}

#[test]
fn a_total_function_may_not_reach_a_match_it_cannot_prove_covered() {
    // A closed keyword type, every member tried: covered.
    let src = "(defmodule t)\n\
         (sig name ((or :a :b) -> string) :total)\n\
         (defn name (k) (match k (:a \"a\") (:b \"b\")))";
    assert!(file_warnings(src).is_empty(), "{:?}", file_warnings(src));
    // An open scrutinee with no catch-all: the declaration promised a case nothing
    // establishes.
    let src2 = "(defmodule t)\n\
         (sig name (keyword -> string) :total)\n\
         (defn name (k) (match k (:a \"a\") (:b \"b\")))";
    only(
        &file_warnings(src2),
        "t/name is declared :total but a match in its body has no catch-all clause",
    );
    // …and a catch-all settles it.
    let src3 = "(defmodule t)\n\
         (sig name (keyword -> string) :total)\n\
         (defn name (k) (match k (:a \"a\") (_ \"other\")))";
    assert!(file_warnings(src3).is_empty(), "{:?}", file_warnings(src3));
}

#[test]
fn properties_survive_the_heap_store_beside_the_type() {
    // A loaded module's `(sig f T :pure)` is read back as both a type and a property,
    // whichever order the two halves were registered in.
    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "(defmodule pp) (sig f :pure) (sig f (int -> int)) (defn f (x) x) \
             (sig g (int -> int) :total :pure) (defn g (x) x)",
        )
        .expect("load");
    let heap = &interp.heap;
    let f = crate::core::value::intern("pp/f");
    let g = crate::core::value::intern("pp/g");
    let props = |s| {
        crate::builtins::modules::sig_props_of(heap, heap.declared_sig_value(s))
            .iter()
            .map(|k| crate::core::value::symbol_name(*k))
            .collect::<Vec<_>>()
    };
    assert_eq!(props(f), vec!["pure"]);
    assert_eq!(props(g), vec!["total", "pure"]);
    assert_eq!(
        super::super::sigs::declared_heap_sig(heap, f).map(|s| s.to_string()),
        Some("(int) -> int".to_string())
    );
    assert_eq!(
        super::super::sigs::declared_heap_sig(heap, g).map(|s| s.to_string()),
        Some("(int) -> int".to_string())
    );
}

#[test]
fn a_counter_climbing_under_a_bound_terminates() {
    // `(+ i 1)` under `(< i n)` where `n = (count codes)` — the count relation (ADR-350)
    // makes `i` an index of an immutable `codes`, so `(count codes) - i` descends.
    let src = "(defmodule t)\n\
         (sig walk :total)\n\
         (defn- walk (codes n i acc) (if (>= i n) acc (walk codes n (+ i 1) (+ acc (nth codes i)))))\n\
         (defn f (s) (let (codes (string/->codepoints s) n (count codes)) (walk codes n 0 0)))\n\
         (sig to-ten (int -> int) :total)\n\
         (defn to-ten (i) (if (< i 10) (to-ten (inc i)) i))";
    assert!(file_warnings(src).is_empty(), "{:?}", file_warnings(src));
    // …and with `n` unrelated to anything, nothing bounds `i` above.
    let src2 = "(defmodule t)\n\
         (sig climb (int int -> int) :total)\n\
         (defn climb (i n) (if (>= i n) i (climb (+ i 1) n)))";
    only(
        &file_warnings(src2),
        "t/climb is declared :total but a self-call hands no parameter a structural decrease",
    );
}
