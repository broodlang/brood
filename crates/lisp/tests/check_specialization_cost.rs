//! The advisory checker must walk each call-site specialization question ONCE (KI-138).
//!
//! `brood file.blsp` type-checks the program before running it, so every millisecond the
//! checker spends is paid on every invocation — and the cost is invisible from inside the
//! language. `specialized_ret` re-types a callee's body under the call's argument types and
//! memoizes the answer per `(name, argument types)`; a `None` reached by an arm that cannot
//! be typed used to return through `?` BEFORE that memo write, so the question was re-asked
//! at every call site and every enclosing level, re-walking the body each time.
//!
//! On a two-line file calling `supervisor/start` — a map-heavy module — that made the
//! prelude's `get` re-typed **1864 times for 52 distinct questions**, and the file's check
//! cost 470 ms against 41 ms before the specializer existed. The `supervisor` row of the
//! cross-language benchmark read +50% for it, which is how it was found; nothing in the
//! runtime was slower.
//!
//! The bound below is deliberately loose — it is not a wall-clock budget and it is not a
//! precision assertion. It fails on a RATIO regression (a re-ask multiplier returning),
//! not on the module gaining a few functions. Measured at the fix: 238 spent. Before it:
//! 2760.

use brood::Interp;

/// `(arm re-typings, expression visits)` a check of `src` spends.
fn meters_for(src: &str) -> (u32, u64) {
    let mut interp = Interp::new();
    let forms = brood::syntax::reader::read_all_positioned(&mut interp.heap, src)
        .unwrap_or_else(|e| panic!("the probe file must parse: {e}"));
    let just: Vec<_> = forms.into_iter().map(|(f, _)| f).collect();
    brood::types::check::check_file(&mut interp.heap, &just);
    (
        brood::types::check::specialization_fuel_spent(),
        brood::types::check::expr_ty_visits_spent(),
    )
}

/// Arm re-typings a check of `src` spends.
fn fuel_for(src: &str) -> u32 {
    meters_for(src).0
}

/// A body of `depth` nested `let`-in-`do` levels whose every binding is UNKNOWN (an
/// unbound callee), so no level can be typed. Before the KI-139 fix each unknown
/// control-flow level fell through to the call path and had its "arguments" re-typed,
/// doubling the walk below it: 2^depth visits for a body of ~6·depth nodes.
fn nested_unknown_control_flow(depth: usize) -> String {
    let mut body = String::from("(opaque x)");
    for i in 0..depth {
        body = format!(
            "(let (v{i} (opaque {})) (do {body}))",
            if i == 0 { "x" } else { "v0" }
        );
    }
    format!("(defmodule cost-probe-nested)\n(defn f (x) {body})\n")
}

#[test]
fn an_unknown_control_flow_level_is_typed_once_not_as_a_call_to_its_own_head() {
    // KI-139. `(do X)` with `X` unknown is unknown; it is not a call to a function named
    // `do` whose argument `X` deserves a second typing for specialization. Linear in the
    // nesting depth, so doubling the depth must not square the visits.
    let (_, shallow) = meters_for(&nested_unknown_control_flow(6));
    let (_, deep) = meters_for(&nested_unknown_control_flow(12));
    eprintln!("nested unknown control flow: depth 6 → {shallow} visits, depth 12 → {deep}");
    assert!(
        deep < shallow * 4,
        "typing 12 nested unknown `let`/`do` levels cost {deep} expression visits against \
         {shallow} for 6 — a re-walk per level (2^depth) is back: a control-flow form the \
         inferencer cannot type must return unknown, never fall through to the call path"
    );
}

#[test]
fn a_call_into_a_map_heavy_module_asks_each_question_once() {
    // `supervisor` is the module that exposed this: 31 mutually-calling private functions
    // passing maps around, so the same `(get <precise map> <keyword>)` question arises at
    // many levels of one walk.
    let (spent, visits) = meters_for("(defmodule cost-probe)\n(def sup (supervisor/start []))\n");
    eprintln!("supervisor/start probe: {spent} arm re-typings, {visits} expression visits");
    assert!(
        visits < 60_000,
        "checking a call to `supervisor/start` made {visits} expression visits; it made \
         ~240 000 when every unknown control-flow level was re-typed as a call to its own \
         head (KI-139), and the fix reads well under 60 000. A multiple of that is a re-walk."
    );
    assert!(
        spent < 800,
        "checking a call to `supervisor/start` spent {spent} arm re-typings; it spends 238 \
         when each distinct question is walked once, and 2760 when a `None` escapes the \
         specialization memo (KI-138). A number in the thousands means the memo is being \
         bypassed again, not that `supervisor` grew."
    );
}

#[test]
fn a_file_that_specializes_nothing_spends_nothing() {
    // The counter has to be able to read LOW, or the bound above passes for the wrong
    // reason — a specializer that never runs would satisfy it too.
    let spent = fuel_for("(defmodule cost-probe)\n(io/puts 0)\n");
    assert!(
        spent <= 4,
        "a file that calls nothing spent {spent} arm re-typings — the meter is not measuring \
         what the bound above assumes"
    );
}
