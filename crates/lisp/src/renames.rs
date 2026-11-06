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
    // ADR-630 — the editor toolkit split along its real seams: a module that claims to be pure
    // is pure, and a thin client does not load the loop. Every public name that left its module
    // points at its new home, so bedit's `nest check` names where it went.
    (
        "editor/buffer/spawn-buffer",
        "editor/buffer-server/spawn-buffer",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-edit",
        "editor/buffer-server/buffer-edit",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-edit-reply",
        "editor/buffer-server/buffer-edit-reply",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-edit-value",
        "editor/buffer-server/buffer-edit-value",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-splice",
        "editor/buffer-server/buffer-splice",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-marker-move",
        "editor/buffer-server/buffer-marker-move",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-query",
        "editor/buffer-server/buffer-query",
        "ADR-630",
    ),
    (
        "editor/buffer/subscribe-buffer",
        "editor/buffer-server/subscribe-buffer",
        "ADR-630",
    ),
    (
        "editor/buffer/unsubscribe-buffer",
        "editor/buffer-server/unsubscribe-buffer",
        "ADR-630",
    ),
    (
        "editor/buffer/buffer-sync",
        "editor/buffer-server/buffer-sync",
        "ADR-630",
    ),
    (
        "editor/buffer/stop-buffer",
        "editor/buffer-server/stop-buffer",
        "ADR-630",
    ),
    (
        "editor/buffer/create-buffer",
        "editor/buffer-server/create-buffer",
        "ADR-630",
    ),
    (
        "editor/buffer/delete-buffer",
        "editor/buffer-server/delete-buffer",
        "ADR-630",
    ),
    ("editor/serve/attach", "editor/attach/attach", "ADR-630"),
    (
        "editor/serve/attach-display",
        "editor/attach/attach-display",
        "ADR-630",
    ),
    (
        "editor/serve/attach-display-local",
        "editor/attach/attach-display-local",
        "ADR-630",
    ),
    (
        "editor/serve/serve-name",
        "editor/serve-protocol/serve-name",
        "ADR-630",
    ),
    (
        "editor/ui/*term-display*",
        "editor/frontend/*term-display*",
        "ADR-630",
    ),
    (
        "editor/ui/*osc52-max-bytes*",
        "editor/frontend/*osc52-max-bytes*",
        "ADR-630",
    ),
    (
        "editor/ui/display-polls",
        "editor/frontend/display-polls",
        "ADR-630",
    ),
    (
        "editor/ui/term-signal",
        "editor/frontend/term-signal",
        "ADR-630",
    ),
    (
        "editor/ui/display-available?",
        "editor/frontend/display-available?",
        "ADR-630",
    ),
    (
        "editor/ui/term-clipboard-set",
        "editor/frontend/term-clipboard-set",
        "ADR-630",
    ),
    (
        "editor/ui/osc52-fits?",
        "editor/frontend/osc52-fits?",
        "ADR-630",
    ),
    (
        "editor/ui/display-poll-input",
        "editor/frontend/display-poll-input",
        "ADR-630",
    ),
    (
        "editor/ui/display-poll-tagged",
        "editor/frontend/display-poll-tagged",
        "ADR-630",
    ),
    (
        "editor/ui/display-texture",
        "editor/frontend/display-texture",
        "ADR-630",
    ),
    (
        "editor/ui/display-sound",
        "editor/frontend/display-sound",
        "ADR-630",
    ),
    (
        "editor/ui/gui-display",
        "editor/frontend/gui-display",
        "ADR-630",
    ),
    (
        "editor/ui/display-key-held?",
        "editor/frontend/display-key-held?",
        "ADR-630",
    ),
    (
        "editor/ui/display-broadcast",
        "editor/frontend/display-broadcast",
        "ADR-630",
    ),
    (
        "editor/ui/with-events",
        "editor/frontend/with-events",
        "ADR-630",
    ),
    (
        "editor/ui/quit-request?",
        "editor/frontend/quit-request?",
        "ADR-630",
    ),
    (
        "editor/ui/focus-request?",
        "editor/frontend/focus-request?",
        "ADR-630",
    ),
    (
        "editor/ui/hangup-request?",
        "editor/frontend/hangup-request?",
        "ADR-630",
    ),
    // ADR-630 block end
    // ADR-506 — the bare namespace is the language: syntax, a measured core and the
    // built-in kinds. Everything else is qualified by its family.
    ("*units*", "test/*units*", "ADR-506"),
    ("*ui-slow-reports*", "editor/ui/*slow-reports*", "ADR-506"),
    ("*ui-memo*", "editor/ui/*memo*", "ADR-506"),
    ("*ui-loop*", "editor/ui/*loop*", "ADR-506"),
    ("*test-wait-ms*", "test/*wait-ms*", "ADR-506"),
    ("*test-trace*", "test/*trace*", "ADR-506"),
    ("*test-timeout-ms*", "test/*timeout-ms*", "ADR-506"),
    (
        "*test-slow-timeout-ms*",
        "test/*slow-timeout-ms*",
        "ADR-506",
    ),
    ("*test-report-sink*", "test/*report-sink*", "ADR-506"),
    ("*test-max-failures*", "test/*max-failures*", "ADR-506"),
    ("*test-last-ran*", "test/*last-ran*", "ADR-506"),
    ("*test-last-failed*", "test/*last-failed*", "ADR-506"),
    ("*test-filter*", "test/*filter*", "ADR-506"),
    (
        "*term-display*",
        "editor/frontend/*term-display*",
        "ADR-630",
    ),
    ("*stale-temp-age-ms*", "test/*stale-temp-age-ms*", "ADR-506"),
    (
        "*resolver-time-budget-ms*",
        "resolver/*time-budget-ms*",
        "ADR-506",
    ),
    ("*resolver-max-steps*", "resolver/*max-steps*", "ADR-506"),
    ("*repl-prompt*", "repl/*prompt*", "ADR-506"),
    ("*repl-interruptible*", "repl/*interruptible*", "ADR-506"),
    ("*repl-cont-prompt*", "repl/*cont-prompt*", "ADR-506"),
    ("*project-width*", "project/*width*", "ADR-506"),
    ("*project-version*", "project/*version*", "ADR-506"),
    ("*project-test-paths*", "project/*test-paths*", "ADR-506"),
    ("*project-templates*", "scaffold/*templates*", "ADR-506"),
    (
        "*project-source-paths*",
        "project/*source-paths*",
        "ADR-506",
    ),
    ("*project-root*", "project/*root*", "ADR-506"),
    ("*project-repository*", "project/*repository*", "ADR-506"),
    ("*project-name*", "project/*name*", "ADR-506"),
    (
        "*project-main-override*",
        "project/*main-override*",
        "ADR-506",
    ),
    ("*project-main*", "project/*main*", "ADR-506"),
    ("*project-kind*", "project/*kind*", "ADR-506"),
    (
        "*project-format-paths*",
        "project/*format-paths*",
        "ADR-506",
    ),
    ("*project-enhances*", "project/*enhances*", "ADR-506"),
    ("*project-doc-paths*", "project/*doc-paths*", "ADR-506"),
    (
        "*project-dev-dependencies*",
        "project/*dev-dependencies*",
        "ADR-506",
    ),
    ("*project-description*", "project/*description*", "ADR-506"),
    (
        "*project-dependencies*",
        "project/*dependencies*",
        "ADR-506",
    ),
    (
        "*project-bundled-packages*",
        "project/*bundled-packages*",
        "ADR-506",
    ),
    ("*project-brood*", "project/*brood*", "ADR-506"),
    ("*parallel-batch*", "test/*parallel-batch*", "ADR-506"),
    ("*observe-timeout*", "observer/*timeout*", "ADR-506"),
    ("*observe-keymap*", "observer/*keymap*", "ADR-506"),
    ("*ns-package*", "project/*ns-package*", "ADR-506"),
    ("*mcp-progress-sink*", "mcp/*progress-sink*", "ADR-506"),
    ("*mcp-documents*", "mcp/*documents*", "ADR-506"),
    ("*loaded-mtimes*", "project/*loaded-mtimes*", "ADR-506"),
    ("*lineedit-keymap*", "editor/lineedit/*keymap*", "ADR-506"),
    (
        "*http-max-response-bytes*",
        "http/*max-response-bytes*",
        "ADR-506",
    ),
    ("*http-max-head-bytes*", "http/*max-head-bytes*", "ADR-506"),
    ("*http-max-body-bytes*", "http/*max-body-bytes*", "ADR-506"),
    ("*fuzzy-workers*", "fuzzy/*workers*", "ADR-506"),
    (
        "*fuzzy-worker-timeout-ms*",
        "fuzzy/*worker-timeout-ms*",
        "ADR-506",
    ),
    ("*fuzzy-scorer*", "fuzzy/*scorer*", "ADR-506"),
    ("*fuzzy-parallel-min*", "fuzzy/*parallel-min*", "ADR-506"),
    ("*format-pair-body*", "format/*pair-body*", "ADR-506"),
    (
        "*format-headers-extra*",
        "format/*headers-extra*",
        "ADR-506",
    ),
    ("*faces*", "editor/face/*faces*", "ADR-506"),
    (
        "*error-explanations*",
        "explain/*error-explanations*",
        "ADR-506",
    ),
    ("*debug-session*", "debug/*session*", "ADR-506"),
    (
        "*config-registry-token*",
        "project/*config-registry-token*",
        "ADR-506",
    ),
    ("*config-registry*", "project/*config-registry*", "ADR-506"),
    ("*config-git-init*", "project/*config-git-init*", "ADR-506"),
    (
        "*comment-kinds*",
        "editor/treesit/*comment-kinds*",
        "ADR-506",
    ),
    ("time-of-day?", "datetime/time-of-day?", "ADR-506"),
    ("datetime?", "datetime/datetime?", "ADR-506"),
    ("date?", "datetime/date?", "ADR-506"),
    ("multimap?", "multimap/multimap?", "ADR-506"),
    ("pq?", "pq/pq?", "ADR-506"),
    ("queue?", "queue/queue?", "ADR-506"),
    ("read-line", "io/read-line", "ADR-506"),
    ("pr-str-bounded", "debug/pr-str-bounded", "ADR-506"),
    ("tap", "debug/tap", "ADR-506"),
    ("spy", "debug/spy", "ADR-506"),
    (
        "reserved-package-name?",
        "reflect/reserved-package-name?",
        "ADR-506",
    ),
    ("multi-return-type", "reflect/multi-return-type", "ADR-506"),
    ("gensym", "reflect/gensym", "ADR-506"),
    ("macroexpand-1", "reflect/macroexpand-1", "ADR-506"),
    ("macroexpand", "reflect/macroexpand", "ADR-506"),
    ("doc-search", "reflect/doc-search", "ADR-506"),
    ("doc", "reflect/doc", "ADR-506"),
    ("arglist", "reflect/arglist", "ADR-506"),
    ("apropos", "reflect/apropos", "ADR-506"),
    ("offload", "proc/offload", "ADR-506"),
    ("eval", "reflect/eval", "ADR-290"),
    ("eval-string", "reflect/eval-string", "ADR-290"),
    ("load", "reflect/load", "ADR-290"),
    ("sleep", "proc/sleep", "ADR-506"),
    ("disj", "set/disj", "ADR-506"),
    ("update-in", "map/update-in", "ADR-506"),
    ("get-in", "map/get-in", "ADR-506"),
    ("dissoc-in", "map/dissoc-in", "ADR-506"),
    ("assoc-in", "map/assoc-in", "ADR-506"),
    ("sort-by", "seq/sort-by", "ADR-506"),
    ("repeat", "seq/repeat", "ADR-506"),
    ("partition", "seq/partition", "ADR-506"),
    ("mapv", "seq/mapv", "ADR-506"),
    ("take-while", "seq/take-while", "ADR-506"),
    ("drop-while", "seq/drop-while", "ADR-506"),
    ("but-last", "seq/but-last", "ADR-506"),
    ("cdddr", "list/cdddr", "ADR-506"),
    ("cddar", "list/cddar", "ADR-506"),
    ("cdadr", "list/cdadr", "ADR-506"),
    ("cdaar", "list/cdaar", "ADR-506"),
    ("caddr", "list/caddr", "ADR-506"),
    ("cadar", "list/cadar", "ADR-506"),
    ("caadr", "list/caadr", "ADR-506"),
    ("caaar", "list/caaar", "ADR-506"),
    ("cddr", "list/cddr", "ADR-506"),
    ("cdar", "list/cdar", "ADR-506"),
    ("cadr", "list/cadr", "ADR-506"),
    ("caar", "list/caar", "ADR-506"),
    ("complement", "combinator/complement", "ADR-506"),
    ("constantly", "combinator/constantly", "ADR-506"),
    ("juxt", "combinator/juxt", "ADR-506"),
    ("fnil", "combinator/fnil", "ADR-506"),
    // ADR-433 — `gen/start` takes options; the two one-line wrappers that did not compose
    // are gone. The target is a CALL SHAPE, not a name: rewriting `gen/start-link` to bare
    // `gen/start` would silently drop the link, so `--fix-renames` leaves any target that
    // is not a bare name to a human.
    (
        "gen/start-link",
        "gen/start f state {:link true}",
        "ADR-433",
    ),
    (
        "gen/start-named",
        "gen/start f state {:name name}",
        "ADR-433",
    ),
    // ADR-398 — `seq/vector-ref` and `seq/vector-length` were `nth` and `count` under
    // other names, so they are deleted. One difference survives the rename: `nth` answers
    // `nil` out of range where `seq/vector-ref` threw.
    ("seq/vector-ref", "nth", "ADR-398"),
    ("seq/vector-length", "count", "ADR-398"),
    // ADR-356 — the Markdown renderer is a CORE module. `docs` is developer tooling and a
    // shipped runtime does not carry it; the hosted registry rendered package READMEs with
    // `docs/markdown->html` at request time, which only worked because its runtime was not
    // as lean as its build command claimed.
    ("docs/markdown->html", "markdown/->html", "ADR-356"),
    // ADR-330 — `filter` joins its complement in `seq/`. The positive was the last core
    // sequence op whose negative lived in a different namespace, so the pair is reunited by
    // moving the positive rather than duplicating it: bare `filter` is gone, `seq/remove` is
    // `seq/reject`, and the transducer/lazy/vector layers follow the same spelling.
    //
    // The bare `filter` entry is the one that matters most here: it is a prelude name every
    // downstream file uses, so without it the whole wave lands as `unbound symbol: filter`
    // with nowhere to go. `--fix-renames` applies it unambiguously.
    ("filter", "seq/filter", "ADR-330"),
    ("seq/remove", "seq/reject", "ADR-330"),
    ("seq/xremove", "seq/xreject", "ADR-330"),
    ("seq/lremove", "seq/lreject", "ADR-330"),
    // ADR-325 — `std/tool/project.blsp` split into `project` (the model), `project-image`,
    // `project-check`, `project-run` and `project-release`; every public name that left
    // the model module is pointed at its new home (bedit's `about.blsp` was the first
    // caller to hit `project/build-info`). `source-files` went back: it is the model's
    // question, which six tools were each answering by hand, and the image only sorts it.
    (
        "project-image/source-files",
        "project/source-files",
        "ADR-325",
    ),
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
        "project-run/run-named-tests",
        "ADR-325",
    ),
    (
        "project-run/run-loaded-tests",
        "project-run/run-named-tests",
        "ADR-339",
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
