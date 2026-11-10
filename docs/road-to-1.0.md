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
  - 🟡 **Thread 6, "reduce before adding": decided (ADR-674), stage 1 landed.** `doseq` is the
    one form for effect and collects nothing. `dotimes`, `dolist`, `unless` and `match*` are
    deprecated: they warn, and `nest format --migrate` rewrites them. `match*` became the internal
    `%match`. `case`, `with-out-str` and `with-err-str` stay. **Stage 2, before `rc.0`:** remove
    the four forms and their migrations, and put each in the rename ledger.
  - **Thread 5, a declared shadow for root functions.** Adding the warning later would be a new
    lint. Under ADR-673 §5 that is advisory and so additive, but only once structured
    severities exist (B3). Otherwise decide it now.
  - **Thread 10, operators that can be shadowed locally.** This is a relaxation, so it is
    additive and can wait.
  - **Threads 1, 3, 4, 7, 8, 9 and 11** are additive (warning controls, regex groups, URL
    resolution, the non-tail lint, view patterns, parametric abilities, function modifiers).
    They can come after 1.0, except that thread 1 is needed by B3.
- ⬜ **Re-ratify ADR-170.** Two rows are stale. The "nominal types" row says `type-of` is
  `:map`, which ADR-542 superseded. Continuations are refused in practice, but the table does
  not list them. Restate the table as it stands at 1.0.

## B. Mechanism the promise needs (kernel and std)

1. ⬜ **A format version on the lock file.** Today `(lock :dependencies …)` has none, and a lock
   it cannot read is treated as absent (`std/tool/package.blsp`). Add the version, refuse a
   newer one with a message, and read every older one.
2. ⬜ **Manifest keys.** An unknown `project.blsp` key should warn, never fail, so that a later
   minor can add keys. Check what happens today and pin it with a test.
3. ⬜ **Structured warning severities in `nest check`.** Replace the matching on message text
   (`project-check.blsp`'s "for now, pending structured warning severities") with a severity on
   each warning. Then add project- and file-level controls (open thread 1), the "new lint is
   advisory" rule, and the `project.blsp` key that opts a project into new lints.
   ADR-673 §5 depends on this.
4. ⬜ **Wire protocol versioning (ADR-673 §3: same `major.minor`).** Put the Brood
   `major.minor` in the handshake beside the protocol magic. Refuse a mismatch with a message
   that names both versions. Keep the refusal before any session key is derived, so a
   mismatched node learns why and nothing else.
5. ⬜ **Deprecating CLI flags.** `nest`'s flags are data in `std/tool/nest.blsp`, so they can
   carry `:deprecated` the way names do. Today nothing can retire a flag gently.
6. 🟡 **Stability metadata in use.** `:since`, `:deprecated` and `:beta` exist (ADR-283), and
   `std/` uses them about 22 times. Nothing is `:beta` at 1.0, so clear the three existing
   `:beta` marks before the release candidates: settle each name or remove it. `:since` stays
   implicit for names that exist at 1.0. Make `nest doc` show "beta" for names added later.
7. ⬜ **"An error keeps its code."** `error-codes.md` promises only that a code is never reused.
   Add a gate that fails if an existing code's `:kind` changes.

## C. The docs tell the truth

- ⬜ **README.** Replace the "experimental, pre-1.0" warning with the ADR-673 promise. Link
  `brood-among-lisps.md`.
- ⬜ **`language.md`.** Its paragraph on "most-specific arity wins" (ADR-047) contradicts
  ADR-619's in-order clauses further down the same page. Sweep the page against the
  implementation once, since it is the document a newcomer trusts.
- ⬜ **`lsp.md`** still talks about a `require` target; there is no `require` form.
- ⬜ **CLAUDE.md** loses the greenfield section at 1.0.0 (ADR-673 §7). The rename-wave section
  becomes the procedure for a major release.
- ⬜ **A "Coming from …" note for Common Lisp and Scheme**, beside the Clojure table. The
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
- Land ADR-674's stage 2: remove `dotimes`, `dolist`, `unless` and `match*`, with ledger entries.
- Rename anything the surface pass finds to be misnamed. A rename after 1.0 costs a full
  deprecation window.

## Out of scope for 1.0 (additive, ADR-011)

These are left out because they can be added later without breaking anyone, not because they
don't matter:

- **Language threads:** the additive ones from A.
- **General-purpose gaps:** 3 (build-on-fetch and a C embedding API), 4 (HTTP/2, gRPC), 5 to 7
  (packages, the OTLP exporter, DAP polish), and 9 and 10 (footprint and scale).
