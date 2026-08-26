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
    let opening_quote = after_key.find('"').expect(":version is followed by a string");
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
