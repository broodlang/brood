//! The Brood manifest and the Cargo workspace both carry this repo's version, and
//! nothing derives one from the other. That drifted silently: `project.blsp` still
//! said `0.1.0` at Cargo's `0.13.0` — twelve releases of a manifest naming a version
//! that never shipped. It is cosmetic (only `nest test` on this repo reads it), which
//! is exactly why nobody noticed.
//!
//! Cheaper to assert than to remember.

/// `:version "…"` as written in the repo's own `project.blsp`.
fn manifest_version() -> String {
    let manifest = include_str!("../../../project.blsp");
    let after_key = manifest
        .split_once(":version")
        .expect("project.blsp declares :version")
        .1;
    let opening_quote = after_key
        .find('"')
        .expect(":version is followed by a string");
    let rest = &after_key[opening_quote + 1..];
    let closing_quote = rest.find('"').expect(":version's string is terminated");
    rest[..closing_quote].to_string()
}

#[test]
fn version_matches_cargo() {
    assert_eq!(
        manifest_version(),
        env!("CARGO_PKG_VERSION"),
        "project.blsp's :version disagrees with the Cargo workspace version — \
         bump both when releasing"
    );
}

/// The `(system/brood-version)` line of `std/system.blsp`'s module docstring.
///
/// That docstring is rendered on the hosted reference page (introspected from the
/// runtime), and its `;; =>` form is NOT the ` -> ` shape `doc_examples_test`
/// executes — so no test ran it and nothing here compared it.
fn system_docstring_example() -> String {
    let source = include_str!("../../../std/system.blsp");
    source
        .lines()
        .find(|line| line.contains("(system/brood-version)"))
        .expect("std/system.blsp's docstring shows (system/brood-version)")
        .to_string()
}

/// The version lives in FOUR places. `.github/workflows/release.yml` compares all of
/// them, but it runs on a pushed TAG — by which time the release is public and the
/// only remedies are moving a tag or burning a version. It fired that way twice:
/// v0.26.0 and v0.27.0 were both tagged and pushed with this line still reading
/// `0.25.2`, and both Release runs died nine seconds in with no binaries built. The
/// first went unnoticed for three days — a tag that publishes nothing looks exactly
/// like a tag from the outside, and the failure is not in the CI run anyone reads.
///
/// The check belongs where it fails BEFORE the tag exists, which is here.
#[test]
fn system_docstring_shows_the_current_version() {
    let example = system_docstring_example();
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        example.contains(version),
        "std/system.blsp's (system/brood-version) example reads\n    {}\nbut the \
         workspace is at {version}. It is rendered on the hosted reference page, and \
         `.github/workflows/release.yml` refuses to build a release while the two \
         disagree — bump it with Cargo.toml and project.blsp, not after tagging.",
        example.trim()
    );
}
