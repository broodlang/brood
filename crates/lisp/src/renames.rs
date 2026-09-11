//! The rename ledger — where a deliberately renamed public name went (ADR-304).
//!
//! A rename wave breaks every downstream caller, and the failure it produces is the
//! least informative one the runtime has: `unbound symbol: gui/font!`. Worse, a caller
//! that wrapped the old name in a blanket `(try … (catch e nil))` never sees even that —
//! bedit ran for hours with ten of ADR-302's renames swallowed exactly that way. The
//! checker knew the names were unbound; nothing knew where they had *gone*, because that
//! fact lived only in `docs/decisions.md`.
//!
//! This table is the single source of truth for that fact. It is Rust — not Brood —
//! because the two consumers that matter most are Rust: the runtime `unbound symbol`
//! error (`eval::unbound_error`) and the checker's unbound diagnostic
//! (`types::check::walk::unbound_msg`), both of which fire before any Brood module can be
//! consulted. Brood tooling reads the same table through the `%renames` primitive
//! (`std/tool/renames.blsp` wraps it), so `nest check --fix-renames` and the runtime never
//! disagree about where a name went. A second table generated from the first was
//! rejected on sight: two sources of truth drift, and a drifted rename hint is worse
//! than none.
//!
//! An entry is `(old, new, adr)`. The old name is spelled exactly as a caller would write
//! it — qualified where the name was public through its module (`gui/font!`), bare where
//! it was a prelude name (`run!`). Only *public* renames belong here: a private
//! (`defn-`) name has no downstream caller to point.

/// `(old, new, adr)` for every deliberate public rename still worth pointing at.
pub const RENAMES: &[(&str, &str, &str)] = &[
    // ADR-325 — `std/tool/project.blsp` split into `project` (the model), `project-image`,
    // `project-check`, `project-run` and `project-release`; every public name that left
    // the model module is pointed at its new home (bedit's `about.blsp` was the first
    // caller to hit `project/build-info`).
    (
        "project/*check-cache-version*",
        "project-check/*check-cache-version*",
        "ADR-325",
    ),
    (
        "project/*check-min-parallel*",
        "project-check/*check-min-parallel*",
        "ADR-325",
    ),
    (
        "project/*check-oversample*",
        "project-check/*check-oversample*",
        "ADR-325",
    ),
    (
        "project/*project-cache-max-age-ms*",
        "project-check/*project-cache-max-age-ms*",
        "ADR-325",
    ),
    (
        "project/*project-cache-pruned*",
        "project-check/*project-cache-pruned*",
        "ADR-325",
    ),
    (
        "project/*project-image-file*",
        "project-image/*project-image-file*",
        "ADR-325",
    ),
    (
        "project/*project-image-registry-exclusions*",
        "project-image/*project-image-registry-exclusions*",
        "ADR-325",
    ),
    ("project/all-files", "project-check/all-files", "ADR-325"),
    (
        "project/build-info",
        "project-release/build-info",
        "ADR-325",
    ),
    (
        "project/build-info-report",
        "project-release/build-info-report",
        "ADR-325",
    ),
    (
        "project/build-stamp-text",
        "project-release/build-stamp-text",
        "ADR-325",
    ),
    (
        "project/bundle-collect",
        "project-release/bundle-collect",
        "ADR-325",
    ),
    ("project/check", "project-check/check", "ADR-325"),
    ("project/check-boot", "project-run/check-boot", "ADR-325"),
    (
        "project/check-bundle-boot",
        "project-release/check-bundle-boot",
        "ADR-325",
    ),
    (
        "project/check-files",
        "project-check/check-files",
        "ADR-325",
    ),
    (
        "project/check-run-closure",
        "project-check/check-run-closure",
        "ADR-325",
    ),
    (
        "project/check-sources",
        "project-check/check-sources",
        "ADR-325",
    ),
    (
        "project/check-structured",
        "project-check/check-structured",
        "ADR-325",
    ),
    (
        "project/file-shadow-warnings",
        "project-check/file-shadow-warnings",
        "ADR-325",
    ),
    (
        "project/fix-renames",
        "project-check/fix-renames",
        "ADR-325",
    ),
    ("project/fix-sigs", "project-check/fix-sigs", "ADR-325"),
    ("project/image-path", "project-image/image-path", "ADR-325"),
    (
        "project/image-prune-foreign-registrations",
        "project-image/image-prune-foreign-registrations",
        "ADR-325",
    ),
    (
        "project/image-usable?",
        "project-image/image-usable?",
        "ADR-325",
    ),
    (
        "project/load-sources-cached",
        "project-image/load-sources-cached",
        "ADR-325",
    ),
    (
        "project/materialize-all",
        "project-image/materialize-all",
        "ADR-325",
    ),
    (
        "project/project-cache-dir",
        "project-check/project-cache-dir",
        "ADR-325",
    ),
    (
        "project/project-entry-fn",
        "project-run/project-entry-fn",
        "ADR-325",
    ),
    (
        "project/project-module-infos",
        "project-check/project-module-infos",
        "ADR-325",
    ),
    (
        "project/project-no-entry-advice",
        "project-run/project-no-entry-advice",
        "ADR-325",
    ),
    (
        "project/project-require-closures",
        "project-check/project-require-closures",
        "ADR-325",
    ),
    (
        "project/release-plain-filename?",
        "project-release/release-plain-filename?",
        "ADR-325",
    ),
    (
        "project/release-plan",
        "project-release/release-plan",
        "ADR-325",
    ),
    (
        "project/release-report",
        "project-release/release-report",
        "ADR-325",
    ),
    (
        "project/release-target-suffix",
        "project-release/release-target-suffix",
        "ADR-325",
    ),
    (
        "project/release-windows-triple?",
        "project-release/release-windows-triple?",
        "ADR-325",
    ),
    ("project/run", "project-run/run", "ADR-325"),
    (
        "project/run-bundle",
        "project-release/run-bundle",
        "ADR-325",
    ),
    (
        "project/run-loaded-tests",
        "project-run/run-loaded-tests",
        "ADR-325",
    ),
    ("project/run-tests", "project-run/run-tests", "ADR-325"),
    (
        "project/run-tests-structured",
        "project-run/run-tests-structured",
        "ADR-325",
    ),
    (
        "project/setup-lazy-image",
        "project-image/setup-lazy-image",
        "ADR-325",
    ),
    (
        "project/source-files",
        "project-image/source-files",
        "ADR-325",
    ),
    (
        "project/suggest-sigs",
        "project-check/suggest-sigs",
        "ADR-325",
    ),
    (
        "project/write-image",
        "project-image/write-image",
        "ADR-325",
    ),
    // ADR-302 — `!` means "raises": every effectful-but-non-raising bang dropped.
    ("run!", "each", "ADR-302"),
    ("gui/title!", "gui/title", "ADR-302"),
    ("gui/icon!", "gui/icon", "ADR-302"),
    ("gui/fullscreen!", "gui/fullscreen", "ADR-302"),
    ("gui/maximize!", "gui/maximize", "ADR-302"),
    ("gui/minimize!", "gui/minimize", "ADR-302"),
    ("gui/font!", "gui/font", "ADR-302"),
    ("gui/inset!", "gui/inset", "ADR-302"),
    ("gui/bg!", "gui/bg", "ADR-302"),
    // The clipboard setter had already moved from `gui/` to `os/` (2026-08); a caller
    // still on the `gui/` spelling gets pointed at the current home, not the interim one.
    ("gui/clipboard-set!", "os/clipboard-set", "ADR-302"),
    ("os/clipboard-set!", "os/clipboard-set", "ADR-302"),
    ("reflect/add-load-path!", "reflect/add-load-path", "ADR-302"),
    ("reflect/set-load-path!", "reflect/set-load-path", "ADR-302"),
    (
        "eval-server/baseline-globals!",
        "eval-server/baseline-globals",
        "ADR-302",
    ),
    ("telemetry/validate!", "telemetry/validate", "ADR-302"),
    ("coverage/begin!", "coverage/begin", "ADR-302"),
    ("coverage/line-begin!", "coverage/line-begin", "ADR-302"),
    ("test/reset-units!", "test/reset-units", "ADR-302"),
    ("test/register-test!", "test/register-test", "ADR-302"),
    // ADR-315 — `some->` deleted: it stopped on `nil`, which since ADR-310 means only "the
    // lookup found nothing" and is an ordinary value everywhere else, so it was a pipe for
    // a channel that is not one. Ledgered rather than left bare because the failure a
    // caller gets is the least informative one we have (`unbound symbol: some->`) and where
    // it went lives only in the ADR — exactly the gap ADR-304 exists to close. (`ok->` and
    // `with` both survive ADR-315; `with` changed shape, which is a semantic break the
    // ledger cannot express — see the ADR.)
    ("some->", "->", "ADR-315"),
];

/// Where `old` went, if it is a ledgered rename: `(new, adr)`.
pub fn renamed_to(old: &str) -> Option<(&'static str, &'static str)> {
    RENAMES
        .iter()
        .find(|(from, _, _)| *from == old)
        .map(|(_, to, adr)| (*to, *adr))
}

/// The suffix both the runtime error and the checker diagnostic append to
/// `unbound symbol: <old>` — one spelling, so the two cannot drift.
pub fn rename_hint(old: &str) -> Option<String> {
    renamed_to(old).map(|(to, adr)| format!(" — renamed to {to} ({adr})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_has_no_duplicate_old_names() {
        let mut seen = std::collections::HashSet::new();
        for (old, _, _) in RENAMES {
            assert!(seen.insert(*old), "duplicate ledger entry for {old}");
        }
    }

    #[test]
    fn hint_names_the_new_spelling_and_the_adr() {
        assert_eq!(
            rename_hint("gui/font!").as_deref(),
            Some(" — renamed to gui/font (ADR-302)")
        );
        assert_eq!(rename_hint("gui/font"), None);
    }
}
