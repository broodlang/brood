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
