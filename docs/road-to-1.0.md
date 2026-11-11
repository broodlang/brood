# Road to 1.0

**Written 2026-11-09 against v0.50.0 + Unreleased.** This is the list of what stands between
the tree and a 1.0.0 that means what it says. The promise itself is ADR-673 (accepted 2026-11-10). How
Brood sits among other Lisps, and so what a newcomer will judge first, is
[`brood-among-lisps.md`](brood-among-lisps.md). The language-surface freeze was done in July
and is archived in [`archive/roadmap-for-v1.md`](archive/roadmap-for-v1.md); this page covers
the rest.

Each item says what "done" looks like and which kind of work it is. Order inside a section is
the order to do it in. Legend: ⬜ not started · 🟡 partly there · ✅ done.

## A. Decide the promise (writing only)

- ✅ **ADR-673 accepted (2026-11-10).**
  - Nodes link within one `major.minor`, and the handshake can be widened later.
  - A project opts into new lints through a key in `project.blsp`.
  - Nothing is carved out as `:beta`. The release candidates are where the surface settles.
  - The minimum time for a deprecation is deferred until closer to 1.0.
- ⬜ **The last surface pass, using the freeze test** (`archive/roadmap-for-v1.md` § The
  test): *can this be added later without breaking anyone, and without leaving the whole corpus
  in a dated idiom?* Run every open thread in `handoff.md` through it:
  - ✅ **Thread 6, "reduce before adding": decided (ADR-674), both stages landed.** `doseq` is
    the one form for effect and collects nothing. `dotimes`, `dolist`, `unless` and `match*` were
    deprecated in stage 1 (2026-11-10) and removed in stage 2 (2026-11-11), with the migration
    that rewrote them; each is in the rename ledger, so its unbound error names the replacement.
    `match*` became the internal `%match`. `case`, `with-out-str` and `with-err-str` stay.
  - **Thread 5, a declared shadow for root functions.** Adding the warning later would be a new
    lint. Under ADR-673 §5 that is advisory and so additive, but only once structured
    severities exist (B3). Otherwise decide it now.
  - **Thread 10, operators that can be shadowed locally.** This is a relaxation, so it is
    additive and can wait.
  - **Threads 1, 3, 4, 7, 8, 9 and 11** are additive (warning controls, regex groups, URL
    resolution, the non-tail lint, view patterns, parametric abilities, function modifiers).
    They can come after 1.0, except that thread 1 is needed by B3.
- ✅ **Re-ratify ADR-170** (2026-11-11): ADR-713 restates the table, every row checked against
  the running language. Ratios and multiple dispatch dropped (shipped), "nominal types" restated
  after ADR-542, continuations added. Two refusals that raised from an internal now name the
  function called (`contains?` on a vector, `map` over a string).

## B. Mechanism the promise needs (kernel and std)

1. ✅ **A format version on the lock file** (2026-11-11). The writer already stamped
   `:version 1`; the reader now honours it — a newer format is refused naming both, an
   unversioned lock reads as 1, a non-lock file is refused. Guard:
   `crates/nest/tests/manifest_and_lock_versions.rs`. Was: Today `(lock :dependencies …)` has none, and a lock
   it cannot read is treated as absent (`std/tool/package.blsp`). Add the version, refuse a
   newer one with a message, and read every older one.
2. ✅ **Manifest keys** (2026-11-11). Today an unknown key was silently ignored; it now warns
   once per command and the project loads. Same guard. Was: An unknown `project.blsp` key should warn, never fail, so that a later
   minor can add keys. Check what happens today and pin it with a test.
3. ✅ **Structured warning severities in `nest check`** (ADR-698, 2026-11-11). Every warning
   carries a category; `nest check` decides gating from it and the project's `:check` key, and a
   `(:check-allow …)` header clause silences a category for a file. Open: the "new lint is
   advisory until the next major" rule needs a lint's `:since`, which only a lint added after
   1.0 will have. Was: Replace the matching on message text
   (`project-check.blsp`'s "for now, pending structured warning severities") with a severity on
   each warning. Then add project- and file-level controls (open thread 1), the "new lint is
   advisory" rule, and the `project.blsp` key that opts a project into new lints.
   ADR-673 §5 depends on this.
4. ✅ **Wire protocol versioning (ADR-673 §3: same `major.minor`)** (ADR-699, 2026-11-11). Was: Put the Brood
   `major.minor` in the handshake beside the protocol magic. Refuse a mismatch with a message
   that names both versions. Keep the refusal before any session key is derived, so a
   mismatched node learns why and nothing else.
5. ✅ **Deprecating CLI flags** (2026-11-11). A flag spec `{:deprecated "VERSION" :use
   "--new"}` keeps working, warns on stderr, stores its value under the replacement, is marked
   in `--help` and drops out of completion. Guard: `tests/nest_test.blsp` "a deprecated flag".
   Was: `nest`'s flags are data in `std/tool/nest.blsp`, so they can
   carry `:deprecated` the way names do. Today nothing can retire a flag gently.
6. ✅ **Stability metadata in use** (2026-11-11). No name in `std/` carries `:beta` (the marks
   this item counted were gone; the only `:beta` left is a docstring example and a test).
   `:since` stays implicit for names that exist at 1.0. A beta name added later shows as beta
   everywhere it surfaces: `nest doc` printed it already, and the editor now does too, in hover
   and in completion's detail line (hover shows a deprecation as well, which it did not).
   Guards: `crates/lsp/src/hover.rs` and `completion.rs` tests.
7. ✅ **"An error keeps its code."** (2026-11-11) `error_codes::KINDS` declares each code's
   kind; `crates/lisp/tests/error_code_kinds.rs` freezes the shipped rows, checks
   `error-codes.md` against them and checks real raises. Was: `error-codes.md` promises only that a code is never reused.
   Add a gate that fails if an existing code's `:kind` changes.

## C. The docs tell the truth

- ✅ **README** (2026-11-11): the warning now says what holds before 1.0 and what ADR-673
  promises from 1.0.0, and links `brood-among-lisps.md`. At 1.0.0 the "until 1.0" half goes.
  Was: Replace the "experimental, pre-1.0" warning with the ADR-673 promise. Link
  `brood-among-lisps.md`.
- ✅ **`language.md`** (2026-11-11). The "most-specific arity wins" paragraph now says clauses
  run in order, and a sweep against the implementation corrected 28 more statements (error
  kinds, removed and renamed names, bitwise width, failure returns, lint severities). It
  found three code bugs, fixed: two `proc/offload` allowlist names left behind by renames,
  a stale `defprotocol` hint, a stale crash-report sample.
- ✅ **`lsp.md`** (2026-11-11): every `require`-form passage (goto, links, completion, the
  add/remove-require code actions, the tier table) now describes what the server does since
  ADR-229. Was: still talks about a `require` target; there is no `require` form.
- ⬜ **CLAUDE.md** loses the greenfield section at 1.0.0 (ADR-673 §7). The rename-wave section
  becomes the procedure for a major release.
- ✅ **A "Coming from …" note for Common Lisp and Scheme** (2026-11-11), `language.md`
  beside the Clojure table, each row checked against the running language. Was: The
  "reaches for and not find" table in `brood-among-lisps.md` is the seed.

## D. It runs where people are

General-purpose gap 2, the release half.

- 🟡 **Platforms.** macOS and Windows smoke sets run in CI, and Windows has been a gate since
  2026-11-04. Remaining: the full suite on macOS, and a decision on whether Windows runs the
  full suite or the smoke set at 1.0. Write that decision down as the supported-platform list.
- ⬜ **Release runtimes.** `release.yml` should publish Windows and musl runtimes, and
  `nest release --target` should download the matching runtime with a hash check instead of
  reading a hand-copied one. This changes ADR-038, so it needs an ADR.
- ⬜ **Install.** A one-line install per platform that needs no Rust toolchain.

## E. Evidence, not features

- ⬜ **`known-issues.md` with no open bug, and the watch item KI-370 resolved or reasoned
  away.**
- ⬜ **`full.yml` green on the release candidate's commit**, together with the weekly soak
  (`soak.yml`) and the two-hour node soak.
- ⬜ **A release-candidate period.** Tag `1.0.0-rc.0`, then `rc.1` and onward. Everything
  is covered from the first candidate. A break between candidates is allowed, but it is
  recorded in the changelog and the ledger, and each one restarts the quiet period. Move `BEDIT_REF` to a bedit that builds
  against it with no deprecation warnings, and let at least one minor's worth of time pass with
  no break. bedit is the canary: it is the largest Brood program there is.

## F. Spend the remaining breaks now

Everything ADR-673 would forbid after 1.0 is cheapest in 0.5x:

- Clear CHANGELOG's Unreleased breaks in a 0.51 release.
- ✅ Land ADR-674's stage 2: remove `dotimes`, `dolist`, `unless` and `match*`, with ledger
  entries (2026-11-11).
- Rename anything the surface pass finds to be misnamed. A rename after 1.0 costs a full
  deprecation window.

## Out of scope for 1.0 (additive, ADR-011)

These are left out because they can be added later without breaking anyone, not because they
don't matter:

- **Language threads:** the additive ones from A.
- **General-purpose gaps:** 3 (build-on-fetch and a C embedding API), 4 (HTTP/2, gRPC), 5 to 7
  (packages, the OTLP exporter, DAP polish), and 9 and 10 (footprint and scale).
