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

/// Arm re-typings a check of `src` spends.
fn fuel_for(src: &str) -> u32 {
    let mut interp = Interp::new();
    let forms = brood::syntax::reader::read_all_positioned(&mut interp.heap, src)
        .unwrap_or_else(|e| panic!("the probe file must parse: {e}"));
    let just: Vec<_> = forms.into_iter().map(|(f, _)| f).collect();
    brood::types::check::check_file(&mut interp.heap, &just);
    brood::types::check::specialization_fuel_spent()
}

#[test]
fn a_call_into_a_map_heavy_module_asks_each_question_once() {
    // `supervisor` is the module that exposed this: 31 mutually-calling private functions
    // passing maps around, so the same `(get <precise map> <keyword>)` question arises at
    // many levels of one walk.
    let spent = fuel_for("(defmodule cost-probe)\n(def sup (supervisor/start []))\n");
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
