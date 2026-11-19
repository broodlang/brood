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

## State (2026-11-19)

- **The front is language features.** Performance, runtime and editor work wait, as ROADMAP's
  design pass set out on 2026-10-28. Not close to a 1.0 rc: never tag one without being told.
- **Released:** v0.56.0 (2026-11-19, tag on `1383f37c`): `defability` is the one contract
  mechanism and `defbehaviour` / `(:implements …)` are gone (ADR-761); an `impl` is checked
  against an ability from another module; checker false positives fixed. The package train
  followed (store 0.9.8, hatch 0.32.0 — live views are ability values —, observe 0.2.7, mail
  0.3.7, …, bedit 0.12.0), and hive and mur-forge were redeployed on it.
- **Green:** per-push CI on `1383f37c`; `full.yml` passed every job on `94a29966` (run
  44473989635) and was dispatched on `1383f37c` (run 44507391905). Per-push CI answers in about
  12 minutes and leaves the sanitizers, Miri, GC stress, soaks and the three suite runs out;
  dispatch `gh workflow run full.yml` and read it with `make green` before the next release.
- **Swept on the test machine 2026-11-18 at `42d573b8`, all clean:** `make test-both`, the fuzz
  differential over all 13 generators, `gcstress-all`, `stress`, `test-contracts`, the
  distribution tests ×5, a 30-minute `runtime-soak`, the ASan kernel suite, and `green-all`'s
  gates (`smoke-bedit` needs a `--features brood/treesit-grammars` nest at `BEDIT_REF`;
  `wasm-test` needs `wasm-bindgen-cli` at the crate's pin).
- **Known issues:** none open. Watching KI-397 (an Interp's parked waiters outlived its drop past
  30 s, once under a full `make test`; the assertion now reports the counts) and KI-395 (a debug-only JIT fast-link mirror assertion, one
  run in 59, cause unknown; the assertion is the armed diagnostic).
- **Waiting on the test machine (`whklat`), not runnable on the desktop:** the
  brood-benchmarks Brood column is still the 0.51.0 measurement, so that repo's daily `smoke`
  is red on its Staleness step. The step-by-step refresh is in brood-benchmarks' `CLAUDE.md`,
  "Refreshing the Brood column".
- **bedit** is pinned by `BEDIT_REF` in `.github/workflows/ci.yml`, which moves often. Smoke with a
  grammar-enabled `nest`, or its mode tests fail on "no grammar", which is not a regression.
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

## Toward 1.0 (2026-11-09)

The owner opened the 1.0 work. The plan is [`road-to-1.0.md`](road-to-1.0.md); the promise is
ADR-673 (accepted 2026-11-10; the deprecation window's minimum time set 2026-11-17: one minor
that warns, six months to the removing major); the positioning it starts from is
[`brood-among-lisps.md`](brood-among-lisps.md). Thread 6 below was the one that had to be settled
before 1.0, because it removes forms: ADR-674 deprecated `dotimes`, `dolist`, `unless` and `match*`
(stage 1, 2026-11-10) and removed them (stage 2, 2026-11-11; the rename ledger names each
replacement). The other threads are additive.

## Open threads: language features, in value order

From the audit of ROADMAP's open items on 2026-11-06; nothing here is scheduled until the owner
picks it. ROADMAP's "Language features — candidates" section is the bar each has to clear, and
ADR-170's refusals (`lazy-seq`, `&key`, `loop`/`recur`, metadata, a character type) stay refused.

1. ~~**Warning suppression at project and file level.**~~ — done by ADR-698 (2026-11-11):
   `project.blsp`'s `:check` key and a `(:check-allow …)` header clause.
2. **A stability policy and the road to 1.0** (`general-purpose-gaps.md`, gap 1).
3. ~~**Regex named groups.**~~ — done by ADR-732 (2026-11-17). Lookaround and backreferences
   stay out: the engine is linear by design (ADR-389).
4. ~~**`std/url`: resolving against a base URL.**~~ — done by ADR-733 (2026-11-17), RFC 3986 §5
   strict. WHATWG stays out.
5. ~~**A declared shadow for root functions.**~~ — done by ADR-735 and ADR-750 (2026-11-17):
   a module defining a root name, or using an import spelled like one bare, declares it with
   `(:shadow name)` or fails compilation. No warning is left.
6. ~~**"Reduce before adding"**~~ — decided by ADR-674; both stages landed (2026-11-11).
7. ~~**The non-tail lint**~~ — deleted by ADR-751 (2026-11-17): a call stack is as deep as
   memory allows, so non-tail recursion is not a hazard to lint. `GcBlockGuard` has no reader:
   audited 2026-11-17 — the five macro-pass sites also hold `MacroBlockGuard`, which does
   suppress collection; the checker's `check_forms` held only it, and its compile-error branch
   read a form from the `forms` slice after a load could collect (now the rooted copy). The
   guard was removed 2026-11-18, with `BROOD_TRACE_GCBLOCK`. A deprecated name used in its
   own file is now never reported to that file, on either path (KI-400).
8. **View patterns** (low to medium value). **Re-assessed 2026-11-19: still deferred, now for
   a measured reason.** No code needs one. A search of std/, tests/ and ten downstream repos
   for the shape a view pattern replaces (a `:when` guard computes `(f x)` and the body
   computes it again) found one hit, a `for` filter in bedit, not a `match`. `(match (f x) …)`,
   a `:when` guard and `with` already say it. A view pattern also hides the value from
   exhaustiveness, since the checker cannot see through `f`. What made it expensive is fixed:
   the Rust side reads patterns through one module now (ADR-755), so a new form costs a
   `ListForm` variant, a corpus entry in `crates/lisp/tests/pattern_grammar.rs`, a compile
   clause in `match.blsp`, and whatever the compiler then points at. A re-run of the search over
   17 repos (128 guards) found six candidates, all `for` filters or incidental repeats.
   **Trigger:** a real `match`, `receive` or `fn` clause that converts a *nested* value in both
   its guard and its body — the one case `(match (f x) …)` cannot say. **If it fires, the form
   is Racket's and `pcase`'s `(app f pat)`**: `f` evaluated with earlier binders in scope (as
   `&optional` defaults, ADR-619), the clause failing only when `pat` fails (no `nil` or
   `failure` special case), the checker counting it as covering nothing, and **refused in
   `receive`**, which re-scans the mailbox and so re-runs it O(k) per scan — the BEAM keeps
   guards to safe built-ins for the same reason. User-defined pattern heads
   (`define-match-expander`, `defpattern`, active patterns) stay out: they make a list
   pattern's meaning depend on a run-time registry, undoing ADR-755's one static reading.
   **The parsing case is answered by string patterns (ADR-770, 2026-11-19)**: `(str "GET " path)`,
   a fixed pure form that is safe in `receive` and gives a compile error for a dead clause —
   what the Erlang and Lisp traditions use instead of a call inside a pattern.
9. **Parametric abilities** (low).
10. ~~**Lexically shadowable operators.**~~ — done by ADR-734 (2026-11-17): a local shadows a
    macro; binding one of the eight evaluator forms fails compilation.
11. **Function modifiers** (wrap, failure-strict calls). **Re-assessed 2026-11-19: still
    deferred.** `docs/deferred.md` §11 names the triggers. One was spent by ADR-316; neither
    other one has fired: no complaint about `number | failure` in hovers (the owner wants
    `failure` to stay in the return type), and no new wrapper. `debug/trace-fn` is arguably a
    third ad-hoc wrapper beside contracts and coverage, but the three work at different levels
    (binding time in the kernel, redefinition with a registry, source instrumentation), so a
    shared `wrap` layer would add a concept without removing much code.

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
