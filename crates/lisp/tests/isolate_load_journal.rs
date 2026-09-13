//! KI-134 / ADR-339 at the kernel level: a write made under `%with-load-journal` — what
//! `require-one` wraps every module load in — survives the `%isolate` restore that would
//! otherwise discard it, while a write the isolate itself made is still rolled back. Both
//! kinds of load-time write are covered: a plain global define and a registry operation
//! (which is journalled as the OPERATION, so the isolate's own registration beside it stays
//! rolled back rather than being resurrected with the map). The second test pins the other
//! contract: `%isolate-discard-loads` is the scratch world the stdlib image builder probes
//! under, where a load rolls back with everything else.
//!
//! Sabotage-verified: with `%with-load-journal` reduced to a plain call of its thunk, the
//! first test's `ki134-kept` is unbound after the isolate.

use brood::Interp;

fn is_true(interp: &mut Interp, form: &str) -> bool {
    let v = interp.eval_str(form).expect("probe form errored");
    interp.print(v) == "true"
}

fn bound(interp: &mut Interp, sym: &str) -> bool {
    is_true(interp, &format!("(bound? '{sym})"))
}

fn feature(interp: &mut Interp, key: &str) -> bool {
    is_true(interp, &format!("(contains? *features* \"{key}\")"))
}

#[test]
fn a_journalled_load_survives_the_isolate_and_the_isolates_own_writes_do_not() {
    let mut interp = Interp::new();
    interp
        .eval_str(
            r#"(%isolate (fn ()
                 (do
                   ;; what a module load does, under the loader's mark
                   (%with-load-journal (fn ()
                     (do (def ki134-kept 1)
                         (%registry-update! '*features* :assoc ["ki134-loaded-mod"] true))))
                   ;; what a test does, in the same window
                   (def ki134-dropped 2)
                   (%registry-update! '*features* :assoc ["ki134-test-mark"] true))))"#,
        )
        .expect("isolate errored");
    assert!(
        bound(&mut interp, "ki134-kept"),
        "the journalled define was rolled back"
    );
    assert!(
        !bound(&mut interp, "ki134-dropped"),
        "the isolate's own define survived — the journal caught a non-load write"
    );
    assert!(
        feature(&mut interp, "ki134-loaded-mod"),
        "the journalled registry op was not replayed"
    );
    assert!(
        !feature(&mut interp, "ki134-test-mark"),
        "the isolate's own registry op survived — a registry replay must re-apply the \
         OPERATION, never the whole map"
    );
    let v = interp
        .eval_str("ki134-kept")
        .expect("kept binding unreadable");
    assert_eq!(interp.print(v), "1");
}

#[test]
fn the_scratch_isolate_rolls_a_journalled_load_back_too() {
    let mut interp = Interp::new();
    interp
        .eval_str(
            r#"(%isolate-discard-loads (fn ()
                 (%with-load-journal (fn ()
                   (do (def ki134-scratch 1)
                       (%registry-update! '*features* :assoc ["ki134-scratch-mod"] true))))))"#,
        )
        .expect("isolate errored");
    assert!(
        !bound(&mut interp, "ki134-scratch"),
        "a scratch isolate must roll a load's define back — the image builder's probes depend on it"
    );
    assert!(
        !feature(&mut interp, "ki134-scratch-mod"),
        "a scratch isolate must roll a load's registry op back"
    );
}
