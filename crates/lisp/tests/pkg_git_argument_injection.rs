//! Regression: a `:git` dependency's URL/ref must never reach `git` as an OPTION.
//!
//! `%git-resolve-ref`, `%git-list-tags` and `%git-clone` put manifest-supplied
//! strings straight into argv. Before the guard in `builtins/pkg.rs`, a dependency
//! declared as
//!
//! ```text
//! (project :dependencies [[evil :git "--upload-pack=touch /tmp/pwned; git-upload-pack"
//!                               :ref "/tmp/somerepo"]])
//! ```
//!
//! made `nest fetch` run `git ls-remote --upload-pack='touch …' /tmp/somerepo`, and
//! git ran the command. That is arbitrary code execution from *reading* a manifest —
//! including a **transitive** dependency's manifest, which the user never sees — with
//! no package code executed and no build script involved. It was verified end to end
//! (`nest fetch` created the file) before the fix.
//!
//! These cases assert the refusal happens in Brood, *before* any process is spawned,
//! so they need no `git` on PATH and touch no network.

use brood::Interp;

/// Every attacker-reachable operand of every git primitive.
const INJECTIONS: &[(&str, &str)] = &[
    // The proven one: `--upload-pack=CMD` makes ls-remote execute CMD.
    (
        r#"(%git-resolve-ref "--upload-pack=touch /tmp/brood-pkg-pwned; git-upload-pack" "/tmp/repo")"#,
        "URL",
    ),
    // The ref is the second operand, so a hostile *ref* beside a benign URL is the
    // same hole from the other side.
    (
        r#"(%git-resolve-ref "/tmp/repo" "--upload-pack=touch /tmp/brood-pkg-pwned")"#,
        "ref",
    ),
    (
        r#"(%git-list-tags "--upload-pack=touch /tmp/brood-pkg-pwned; git-upload-pack")"#,
        "URL",
    ),
    (
        r#"(%git-clone "--upload-pack=id" "/tmp/dest" "main" "0123456789abcdef0123456789abcdef01234567")"#,
        "URL",
    ),
    (
        r#"(%git-clone "https://example.invalid/r.git" "-x" "main" "0123456789abcdef0123456789abcdef01234567")"#,
        "destination",
    ),
    (
        r#"(%git-clone "https://example.invalid/r.git" "/tmp/dest" "--exec=id" "0123456789abcdef0123456789abcdef01234567")"#,
        "ref",
    ),
    (
        r#"(%git-clone "https://example.invalid/r.git" "/tmp/dest" "main" "--exec=id")"#,
        "commit",
    ),
];

#[test]
fn git_primitives_refuse_option_like_operands() {
    let mut interp = Interp::new();
    for (form, what) in INJECTIONS {
        let err = match interp.eval_str(form) {
            Err(e) => e.to_string(),
            Ok(v) => panic!("{form} was accepted (returned {})", interp.print(v)),
        };
        assert!(
            err.contains("refusing") && err.contains(what),
            "{form} must be refused as a hostile {what}, got: {err}"
        );
        // The proof it never reached git: no subprocess could have run, so the
        // canary the payload would have created cannot exist.
        assert!(
            !std::path::Path::new("/tmp/brood-pkg-pwned").exists(),
            "the injected command RAN — {form}"
        );
    }
}

/// The guard must be narrow: an ordinary URL/ref is not refused. Asserted by the
/// shape of the failure — a well-formed URL gets as far as git (or a missing-git
/// error), never the pre-flight refusal.
#[test]
fn ordinary_urls_and_refs_are_not_refused() {
    let mut interp = Interp::new();
    // A nonexistent LOCAL path: git fails fast without touching the network.
    let out = interp.eval_str(r#"(%git-resolve-ref "/nonexistent-brood-repo-xyz" "v1.2.3")"#);
    let text = match out {
        Ok(v) => interp.print(v),
        Err(e) => e.to_string(),
    };
    assert!(
        !text.contains("refusing"),
        "a plain path/ref must not trip the option guard, got: {text}"
    );
}
