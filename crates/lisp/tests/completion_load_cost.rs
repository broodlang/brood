//! `std/tool/complete.blsp` must not load the project image (2026-08-18).
//!
//! Shell completion runs on a keypress, so what that module `require`s is a
//! user-visible cost paid before a single candidate is computed. It opened with
//! `(:use-internals project)` for four twenty-line helpers, which pulled in all 2967
//! lines of `project` — plus `scaffold`, and `project` again behind it — measured at
//! **770 ms of the 950 ms** a completion took in a debug build. That also put
//! `completion_never_fails_however_it_is_called` (96 subprocess spawns) at 64 s on an
//! idle box and past its cap under CI contention (KI-39).
//!
//! The cost was fixed by making the module self-contained, and the value of these
//! tests is that they guard the invariant DIRECTLY. The alternative signal is a
//! wall-clock budget in `.config/nextest.toml`, and that one has already failed twice
//! at this job: it cannot distinguish "the box was busy" from "a dependency came
//! back", it only fires on the machine's worst day, and it says nothing about what
//! broke. A bare `mod/name` reference anywhere in that file silently re-introduces a
//! load-time require (ADR-229 loads by inference), which is a one-token regression —
//! so it gets a one-assert test.
//!
//! A fresh `Interp` is the whole point: it carries the prelude and nothing else, so a
//! global from another std module can only be present because loading `complete` put
//! it there.

use brood::Interp;

/// Is `sym` globally bound in this image?
fn bound(interp: &mut Interp, sym: &str) -> bool {
    let out = interp
        .eval_str(&format!("(bound? '{sym})"))
        .unwrap_or_else(|e| panic!("evaluating (bound? '{sym}) failed: {e}"));
    interp.print(out) == "true"
}

#[test]
fn requiring_complete_does_not_load_the_project_module() {
    let mut interp = Interp::new();
    // Sanity: the name is genuinely absent before the require, so a later `false`
    // means "not loaded" rather than "never existed under this spelling".
    assert!(
        !bound(&mut interp, "project/plist-get"),
        "project was already loaded in a fresh image — this test cannot conclude anything"
    );

    interp
        .eval_str("(require-one 'complete)")
        .expect("load std/tool/complete.blsp");

    // `plist-get` is the specific export that was still being called after the
    // dependency was dropped: unbound, silently swallowed by the module's own
    // `complete-safely` net, and `nest remove <TAB>` quietly offered nothing.
    for name in [
        "project/plist-get",
        "project/find-root",
        "project/collect-sources",
        "project/parse-deps",
    ] {
        assert!(
            !bound(&mut interp, name),
            "requiring `complete` loaded `project` (found {name}) — completion is back to \
             paying ~770 ms of module load on every TAB press. Read what you need off the \
             filesystem instead; see the header of std/tool/complete.blsp."
        );
    }
}

#[test]
fn requiring_complete_does_not_load_the_scaffolder() {
    let mut interp = Interp::new();
    interp
        .eval_str("(require-one 'complete)")
        .expect("load std/tool/complete.blsp");

    // `complete/templates` still needs `scaffold`, but pays for it with a
    // `require-one` INSIDE the function, so only `nest new --template` completion
    // does. Naming an export at top level instead would auto-require it here — and
    // `scaffold` uses `project`, so that one reference alone restores the whole cost.
    assert!(
        !bound(&mut interp, "scaffold/new-project"),
        "requiring `complete` loaded `scaffold` (and so `project` behind it) — keep the \
         template list behind the call-time `require-one` in `complete/templates`."
    );
}

#[test]
fn the_template_completion_still_works_through_the_call_time_require() {
    // The other half of the contract: making the dependency lazy must not make the
    // feature disappear. `scaffold` is absent above; here it must load on demand and
    // the ambient it defines must be readable.
    let mut interp = Interp::new();
    interp
        .eval_str("(require-one 'complete)")
        .expect("load std/tool/complete.blsp");
    let out = interp
        .eval_str("(count (complete/templates))")
        .expect("complete/templates must not raise");
    let count: i64 = interp.print(out).parse().expect("a template count");
    assert!(
        count > 0,
        "complete/templates returned {count} templates — the call-time \
         `(require-one 'scaffold)` no longer binds `*project-templates*`"
    );
}
