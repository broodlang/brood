//! A verdict about every run of a form, judged over every copy the expansion made of it.
//!
//! The `match` compiler splices a short fall-through into each point where a clause can fail
//! (`%match-build-clause`), so one source form can sit in the expansion several times, each
//! copy the SAME heap object under a different scope. `((:a) 1) ((n) :when (int? n) n)`
//! places the second clause both where the first clause's head was not `:a` and where it was
//! `:a` but more arguments followed; in the second copy `n` is `:a`. A finding that holds on
//! one path — an argument of the wrong type — is right to report from one copy. A finding
//! that claims something of EVERY run — "this can never be true" — is right only if every
//! copy says so, and from the one copy it called a guard dead that the other copy needs.
//!
//! [`check_top_level`] opens a ledger around one top-level form's walk; [`report`] records
//! each visit of a judged form and its verdict; when the walk ends, a verdict some visit did
//! not reach is withdrawn.

use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::error::Pos;

/// Per judged form: how often the walk reached it, and the finding each visit that reached
/// the verdict made. The copies are not one heap object — the expander rebuilds each — but
/// they share their source position and their text, which is the key; a macro that writes
/// the same test twice at one position is read as one form, which can only withdraw a
/// finding, never invent one.
#[derive(Default)]
struct Ledger {
    visits: HashMap<(Option<Pos>, String), (usize, Vec<Finding>)>,
}

thread_local! {
    /// The ledger of the top-level walk running on this thread. A walk opens its own and
    /// restores the one it found, so a check started from inside a walk keeps its own.
    static LEDGER: RefCell<Option<Ledger>> = const { RefCell::new(None) };
}

/// Walk one top-level form, withdrawing every verdict about all of a form's runs that some
/// copy of the form did not reach.
pub(in crate::types::check) fn check_top_level(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
    out: &mut Vec<Finding>,
) {
    let outer = LEDGER.with(|ledger| ledger.replace(Some(Ledger::default())));
    check_into(heap, form, ctx, out);
    let ledger = LEDGER
        .with(|ledger| ledger.replace(outer))
        .unwrap_or_default();
    for (seen, findings) in ledger.visits.into_values() {
        if findings.len() == seen {
            continue;
        }
        for finding in findings {
            if let Some(index) = out.iter().position(|reported| *reported == finding) {
                out.remove(index);
            }
        }
    }
}

/// Report `verdict` — a claim about every run of `form` — for this visit of `form`, and
/// record the visit, verdict or not. Outside a [`check_top_level`] walk it is reported as is.
pub(super) fn report(heap: &Heap, form: Value, verdict: Option<Finding>, out: &mut Vec<Finding>) {
    LEDGER.with(|ledger| {
        if let Some(ledger) = ledger.borrow_mut().as_mut() {
            let key = (
                heap.form_pos_only(form),
                crate::syntax::printer::print(heap, form),
            );
            let (seen, findings) = ledger.visits.entry(key).or_default();
            *seen += 1;
            findings.extend(verdict.clone());
        }
    });
    out.extend(verdict);
}
