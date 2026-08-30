//! The prelude's autoload stubs (ADR-246): boot loads nothing, and the first call — which
//! now happens at arbitrary points in a program, typically inside a green process — is
//! sound when many processes race it.
//!
//! Since KI-61 the prelude no longer force-loads `string`/`seq` at boot; it binds the
//! names it references to stubs that `require-one` their module on first call. That moves
//! a module load out of single-threaded boot into ordinary execution. `require-one` is
//! concurrency-safe by construction (a CAS on `*features-loading*`, and a loser waits for
//! the winner's `provide` rather than trusting a top-of-file marker), but that safety was
//! never before exercised *from a stub*, and a partially-loaded module observed through
//! one would return `nil` from a function that should return a value — silently, not as a
//! crash.
//!
//! **The test programs must not NAME a stubbed symbol**, and this is the trap KI-61
//! records: a qualified reference auto-requires its module at *compile* time, so a program
//! mentioning `seq/distinct` has already loaded `seq` before its first line runs, and the
//! stub it meant to exercise is gone. (An earlier version of this file did exactly that and
//! its own precondition caught it.) So each program goes through a **bare-named prelude
//! function whose body reaches a stub** — `reserved-package-name?` calls `seq/distinct`,
//! `doc-search` calls `string/blank?` — which is also the only shape a real first call ever
//! takes: user code that names one of these directly gets the module loaded for it.

use brood::Interp;

/// The direct assertion of what ADR-246 buys, and the guard against losing it: a fresh
/// runtime that has evaluated nothing must have loaded no library module at all. A prelude
/// change that reintroduces a boot-time `require-one` fails here, naming the module.
#[test]
fn boot_loads_no_library_feature() {
    let mut interp = Interp::new();
    // `(keys *features*)` and not a count, so a failure says WHICH module came back.
    let loaded = interp
        .eval_str("(keys *features*)")
        .map(|v| interp.print(v))
        .expect("read *features*");
    // An empty registry prints `nil`, not `()` — `keys` of an empty map is the empty list.
    assert_eq!(
        loaded, "nil",
        "boot loaded a library module. The prelude's references into `string`/`seq` are \
         autoload stubs precisely so boot loads nothing (ADR-246, KI-61) — a force-load \
         here costs every `brood`/`nest`/`brood-lsp` invocation, forever",
    );
}

/// `call` must not name a qualified symbol — see the module comment.
///
/// Runs on whatever load path this process booted with. Since the stdlib image went
/// **default-ON in v0.15.0** that is the image when one is on disk and source when none is —
/// which is exactly the ambiguity `race_first_call_from_the_stdlib_image` exists to remove.
fn race_first_call(module: &str, call: &str, expected: &str) {
    let mut interp = Interp::new();
    run_fan(&mut interp, module, call, expected);
}

/// The same race, with the stdlib image **provably** installed into the racing interpreter.
///
/// This is the arm that guards KI-72, and before it existed that coverage was *accidental*.
/// Nothing in `ci.yml` builds a stdlib image, so in CI these races ran on the source path only
/// — while `image_matches_source.rs` (ADR-280) *does* build one and writes it to
/// `~/.cache/brood`, so whether the race ran imaged depended on which case nextest happened to
/// schedule first. A guard for a race that silently runs on the wrong load path reports
/// "KI-72 is still fixed" without having looked.
///
/// The ADR-280 differential does not close that gap: it compares final *state* — name, kind,
/// privacy, declared signature — and proves the two paths agree once loaded. KI-72 was not a
/// state divergence but an **ordering** one during install, where a section published a public
/// name before binding the module-private helper its body calls. A differential over end state
/// cannot see that by construction.
///
/// Two mechanics make this deterministic rather than dependent on how the process booted:
///
/// 1. **Install explicitly, do not rely on boot.** The shared prelude is built once per process
///    and *inserted* into later `Interp`s rather than re-evaluated, so boot's install decision is
///    fixed by the FIRST interpreter in the process and no env var can change it afterwards.
///    `%std-image-install` is an ordinary function, so calling it here works whatever boot did.
/// 2. **Build in a throwaway interpreter.** `stdimage/build` `require`s every module "for real,
///    so the globals being imaged are live in THIS heap" — which would load `string`/`seq` in
///    the very interpreter that is supposed to race their *first* call. So the build happens
///    somewhere else and only the finished image crosses over.
///
/// Gated on the cargo feature: `./configure --without-stdimage` (ADR-281) ships a build with
/// no image machinery at all, for a deployment that must never touch a cache directory. In such
/// a build these arms cannot install anything, so they must not exist rather than fail — a red
/// test for a supported configuration is a red nobody can act on. `(%build-stdimage?)` is the
/// runtime equivalent if this ever needs checking from Brood.
#[cfg(feature = "stdimage")]
fn race_first_call_from_the_stdlib_image(module: &str, call: &str, expected: &str) {
    // The id carries the git sha and a hash of every baked-in `.blsp`, so any commit or `std/`
    // edit makes the previous image stale. Build on demand rather than asking the developer to
    // remember — a test that needs a manual step is a test that skips itself in CI.
    let mut builder = Interp::new();
    let built = builder
        .eval_str("(or (%std-image-install) (do (stdimage/build) (%std-image-install)))")
        .map(|v| builder.print(v))
        .expect("build/install the stdlib image");
    assert_ne!(
        built, "nil",
        "could not build or install a stdlib image, so this arm would silently re-run the \
         source-path test above and still report success",
    );
    drop(builder);

    let mut interp = Interp::new();
    let installed = interp
        .eval_str("(or (%std-image-install) nil)")
        .map(|v| interp.print(v))
        .expect("install the stdlib image into the racing interpreter");
    assert_ne!(
        installed, "nil",
        "the stdlib image did not install into the racing interpreter, so this test is \
         exercising the SOURCE path and cannot see a KI-72 regression",
    );

    // The race is a race on the FIRST call, so the module must still be unloaded here. If the
    // build leaked into this interpreter (or a future change loads eagerly), the fan would
    // exercise nothing and still pass.
    let loaded = interp
        .eval_str(&format!("(%registry-member? '*features* \"{module}\")"))
        .map(|v| interp.print(v))
        .expect("read *features*");
    assert_eq!(
        loaded, "false",
        "`{module}` was already loaded before the race started, so there is no first call to \
         race and this test would pass without exercising anything",
    );

    run_fan(&mut interp, module, call, expected);
}

/// 24 processes race the first call, and every one must get the same right answer.
fn run_fan(interp: &mut Interp, module: &str, call: &str, expected: &str) {
    let prog = format!(
        r#"
        (def root (self))
        ;; 24 processes reach the stub with no ordering between them: one wins the load
        ;; claim, the rest must WAIT for its provide rather than see a half-loaded module.
        (defn fan (k)
          (do
            (dotimes (_ k) (spawn (send root [:r {call}])))
            (reduce (range k) (list) (fn (acc _) (receive ([:r v] (cons v acc)))))))

        ;; Distinct-count inline rather than through `seq/distinct`, which is itself one of
        ;; the stubs under test — and naming it would load `seq` before the race starts.
        (defn uniq (xs)
          (reduce xs (list) (fn (a x) (if (includes? a x) a (cons x a)))))

        ;; `%registry-member?` and not `system/feature?`: the load happened in a CHILD process,
        ;; and this process's inline cache of `*features*` can still be pre-`provide` —
        ;; the same staleness `require-one` bypasses with this primitive for the same
        ;; reason. `system/feature?` reads false here, which is a cache read, not a lost load.
        (let (got (fan 24))
          [(uniq got) (%registry-member? '*features* "{module}")])
    "#
    );
    let v = interp.eval_str(&prog).unwrap_or_else(|e| {
        panic!(
            "racing the first call into `{module}` errored: {}",
            e.message
        )
    });
    assert_eq!(
        interp.print(v),
        format!("[({expected}) true]"),
        "24 processes racing the first call into `{module}` did not all get the same right \
         answer (every one should be {expected}, and `{module}` should read as loaded)",
    );
}

/// `reserved-package-name?` is bare-named and its body calls `seq/distinct`.
#[test]
fn racing_the_first_call_into_seq_is_sound() {
    race_first_call(
        "seq",
        "(reserved-package-name? 'zzz-not-a-baked-in-module)",
        "false",
    );
}

/// `doc-search` is bare-named and its terms helper calls `string/blank?`. Asserted on
/// emptiness rather than a match count, which would track the docstring corpus.
#[test]
fn racing_the_first_call_into_string_is_sound() {
    race_first_call(
        "string",
        "(empty? (doc-search \"zzzz-no-such-doc\"))",
        "true",
    );
}

// ---- the same races, on the IMAGED path (the default since v0.15.0) ----------------------
//
// A failure here does not look like an assertion about images. It looks like the fan returning
// the wrong list, or the case timing out because a child died before it could reply. If it
// hangs, re-run with `--nocapture`: libtest captures a test's stderr and DISCARDS it when the
// test never completes, which is why the `process N died: unbound symbol: …` line naming the
// cause went unread for two sessions of KI-72.

/// `reserved-package-name?` -> `seq/distinct`, materialised from the stdlib image.
#[cfg(feature = "stdimage")]
#[test]
fn racing_the_first_call_into_seq_is_sound_from_the_stdlib_image() {
    race_first_call_from_the_stdlib_image(
        "seq",
        "(reserved-package-name? 'zzz-not-a-baked-in-module)",
        "false",
    );
}

/// `doc-search` -> `string/blank?` -> the module-private `string/whitespace?`: the exact shape
/// of KI-72, on the exact load path it occurred on.
#[cfg(feature = "stdimage")]
#[test]
fn racing_the_first_call_into_string_is_sound_from_the_stdlib_image() {
    race_first_call_from_the_stdlib_image(
        "string",
        "(empty? (doc-search \"zzzz-no-such-doc\"))",
        "true",
    );
}
