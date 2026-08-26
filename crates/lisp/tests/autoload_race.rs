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
fn race_first_call(module: &str, call: &str, expected: &str) {
    let mut interp = Interp::new();
    let prog = format!(
        r#"
        (def root (self))
        ;; 24 processes reach the stub with no ordering between them: one wins the load
        ;; claim, the rest must WAIT for its provide rather than see a half-loaded module.
        (defn fan (k)
          (do
            (dotimes (_ k) (spawn (send root [:r {call}])))
            (reduce (fn (acc _) (receive ([:r v] (cons v acc)))) (list) (range k))))

        ;; Distinct-count inline rather than through `seq/distinct`, which is itself one of
        ;; the stubs under test — and naming it would load `seq` before the race starts.
        (defn uniq (xs)
          (reduce (fn (a x) (if (includes? a x) a (cons x a))) (list) xs))

        ;; `%registry-member?` and not `feature?`: the load happened in a CHILD process,
        ;; and this process's inline cache of `*features*` can still be pre-`provide` —
        ;; the same staleness `require-one` bypasses with this primitive for the same
        ;; reason. `feature?` reads false here, which is a cache read, not a lost load.
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
