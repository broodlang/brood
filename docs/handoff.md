# Handoff — the current state, and where to pick up

**This page is the current picture only, rewritten in place each session — not a log.** The
narrative and the measurements live in [`devlog.md`](devlog.md) (dated entries, newest at the
bottom); decisions in [`decisions.md`](decisions.md); bugs in
[`known-issues.md`](known-issues.md); the plan in [`../ROADMAP.md`](../ROADMAP.md); the perf
option book in [`runtime-frontier.md`](runtime-frontier.md) and
[`compute-frontier.md`](compute-frontier.md), with how to measure in
[`benchmarking.md`](benchmarking.md). The perf threads and DONE sections this page carried up
to 2026-11-06 moved, unedited, to the end of
[`archive/perf-handoff.md`](archive/perf-handoff.md).

## State (2026-11-06)

- **The front is language features.** Performance, runtime and editor work wait, as ROADMAP's
  design pass set out on 2026-10-28.
- **Released:** v0.50.0 is the version in `Cargo.toml` and the changelog's newest heading, but
  **neither v0.49.1 nor v0.50.0 has a git tag or a GitHub Release**. The latest of both is
  v0.49.0. `CHANGELOG.md`'s "Unreleased" holds ADR-617..620.
- **Green:** `make green` passed on `e30b1cc6` (CI plus its local gates). Per-push CI answers
  in about 12 minutes and leaves out the tree-walker differential, the repeat runs, GC stress,
  the stress suite, the tier audit, the soaks and the sanitizers. Those moved to
  `full.yml` (weekly, or `gh workflow run full.yml`; it replaced the Nightly on 2026-11-05)
  and **`full.yml` had no completed run yet on 2026-11-06**, so the design pass's last commits
  have had per-push CI only. Run it before the next release.
- **Known issues:** nothing open. One watch, **KI-370** (`nest check` warned once on
  `examples/life_bands.blsp`; the diagnostic for the next sighting is in its entry).
- **bedit** is pinned at `df808b2` (`BEDIT_REF`). Smoke with a grammar-enabled
  `nest`, or its mode tests fail on "no grammar", which is not a regression.
- **Before pushing:** `make prepush`, and `git merge --ff-only origin/main` first if it moved.
- **Is the tree green?** `make green`, never by reading the run list.
- **Don't build the debug profile while a suite runs:** `cargo build` relinks
  `target/debug/brood` under the running tests. Release builds are safe.
- **Probes worth keeping:** a program in `unshare -U` with `ulimit -u N` gets an exact thread
  budget (`crates/cli/tests/thread_exhaustion.rs`); `systemd-run --user --scope -p MemoryMax=…`
  gives a real container cap locally.

## Done: the language design pass (2026-10-28 .. 2026-11-06)

ROADMAP's sections A–D are finished (commit `7252319a`, merged as `490a3641`). A doctest engine
that runs markdown too (ADR-498), hygiene at every binding site (ADR-500), one shape for an
error and `catch` taking a pattern (ADR-501), quasiquote levels (ADR-503), a bare namespace
that is the language and nothing else (ADR-506), callback last (ADR-507), every parameter list
a list (ADR-512), `cond`'s `else` (ADR-518), `reduce` raising on empty (ADR-521), no `memoize`
(ADR-522), ability or `defmulti` (ADR-523), records as their own value kind (ADR-542), fields
on `failure` (ADR-617), macro errors at the call or the named sub-form (ADR-618), one grammar
for every `fn` head with in-order clauses (ADR-619), and `()` staying `nil` with a
`:list-truthiness` lint (ADR-620).

Left from that session:
- **ADR numbers collide between parallel rounds.** `git fetch` and read the newest ADR on
  `origin/main` before numbering a new one.
- **Moving the prelude's hand-rolled macro checks onto pattern clauses:** most check a list's
  contents, which a clause head cannot say. Move them as they are touched.
- **pfib at tier 1 moving in `make ab-vm` is not a regression**: it is non-code content baked
  into the binary (the numbers are in the perf archive).

## Open threads: language features, in value order

From the audit of ROADMAP's open items on 2026-11-06; nothing here is scheduled until the owner
picks it. ROADMAP's "Language features — candidates" section is the bar each has to clear, and
ADR-170's refusals (`lazy-seq`, `&key`, `loop`/`recur`, metadata, a character type) stay refused.

1. **Warning suppression at project and file level.** Only `check-allow` around a form exists,
   and any warning fails a batch `nest check`.
2. **A stability policy and the road to 1.0** (`general-purpose-gaps.md`, gap 1).
3. **Regex named groups.** Lookaround and backreferences are missing too (`std/regex.blsp`).
4. **`std/url`: resolving against a base URL.** Still RFC 3986, not WHATWG.
5. **A declared shadow for root functions.** Only a shadowing macro warns (ADR-423).
6. **"Reduce before adding":** four iteration forms and three match forms.
7. **The non-tail lint:** mutual `letrec` recursion, `try` bodies, conditional-position demands.
8. **View patterns** (low to medium value).
9. **Parametric abilities** (low).
10. **Lexically shadowable operators.**
11. **Function modifiers** (wrap, failure-strict calls).

## Flake hunting

A one-in-N race passes N-1 runs; repetition is the evidence. The tools:

- `make gcstress` — the concurrency/mailbox files under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`
  on the debug build (`full.yml` runs it).
- `make test-both` — the suite at the default ceiling and on the tree-walker (`BROOD_TIER=0`).
- `scripts/fuzz/run.sh <generator> [count] [seed]` — the engine differential (tree-walker, VM,
  JIT, GC-stress) over generated programs; `make stress` for the program fuzzer, the reader
  fuzzer and the property/race corpus (`full.yml` runs it).
- `make runtime-soak [SOAK_SECONDS=600]` — boots `stress/runtime_soak.blsp` fresh round after
  round (first module loads raced across a 240-process fan, warm spawn bursts, link churn,
  large copies), cycling tiers, one worker and the GC verifier; stops at the first lost
  process, hang or watchdog line. The hunt for KI-88 and KI-127, whose causes were never
  found; `full.yml` runs it for 30 minutes and keeps every round's record.
- The weekly workflow (`.github/workflows/full.yml`) runs the suite three times, the tree-walker
  differential, the stress suite, ASAN, TSAN + loom, and a `perf-stats` build; `soak.yml` runs the
  node soak for five hours under GC stress.
- A flake hunt keeps its logs on success too: a loop that deletes them when green cannot be
  believed afterwards.
- CI and `full.yml` run the suite through `scripts/nextest-no-flakes.sh`, so a test that
  passes only on nextest's retry fails the job and is named. Before it, `FLAKY 2/2` was green
  and unread (`dns_test` failed its first try in four of six runs before anyone looked).
- Several runtimes in one process is a real configuration (embedders, `cargo test` threads,
  the sanitizer jobs): state that belongs to a runtime lives on `RuntimeCode`, never
  in a `static` (KI-224, KI-226). `crates/lisp/tests/process_wide_statics.rs` makes every
  lock-holding `static` in `crates/lisp/src` state why it is process-wide.
- A test that times out names what it was stuck on: the runner reads the hung worker and the
  processes it spawned before the kill (running / waiting with N unmatched messages / waiting
  with nothing queued).
