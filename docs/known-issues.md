# Known issues

KI-9 is a one-off arity sighting judged a transient inconsistent-build artifact, not
present in committed code; KI-10 no longer reproduces, incidentally fixed — both kept as
records, not open bugs. **KI-17** (the checker reachability gap) is now **FIXED** (ADR-189).
**KI-18** (effect duplication on a deopt) and **KI-19** (call-head evaluation order) are
both now **FIXED**, as is **KI-20** (a fast link ran the callee against the caller's IC
block — a cold cache, never a wrong answer), as is **KI-21** (`nest run --for` /
`--watch` emitted a pre-ADR-150 `~p` pin and failed on every file). **KI-25** (five JIT/VM suites could not be re-run in one image, blocking
`--repeat-until-failure`) is **FIXED** — see its entry for the two fixes and why its original
diagnosis was wrong.
This file is the condensed record — what each was, how it was fixed, and the regression
test that guards it — so a recurrence is recognizable. For the narrative discovery
writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md); deeper rationale is in the cited
ADRs / topic docs.

## Index — status per issue (⌘F the `KI-N` to jump)

| # | What | Status |
|---|---|---|
| KI-36 | `reconnect_watcher_heals_a_fallen_link` failed once at 22.6 s and passed on retry, during a suite run with a 4000-module image build beside it | ⚠️ **watching** (seen once 2026-08-07) |
| KI-35 | `*method-from*` was never imaged, so an imaged start stopped reporting cross-module `defmethod` conflicts | ✅ **fixed** 2026-08-07 |
| KI-34 | the startup image was written on every cold start and **never read from** — two defects, either sufficient | ✅ **fixed** 2026-08-07 |
| KI-33 | fully consuming a stream leaked its producer process — an exhausted stream parked in `stream-done-loop` forever instead of exiting | ✅ **fixed** 2026-08-07 |
| KI-32 | a selective `receive` corrupted a skipped **local** (L1-delivered) message to `nil` — a stream request/reply pipeline deadlocked intermittently | ✅ **fixed** 2026-08-06 |
| KI-31 | a foreign-ecosystem version range compiled to its FIRST term — `">=1.0.0 <2.0.0"` became `>=1.0.0` | ✅ **fixed** 2026-08-06 |
| KI-30 | seven `temp-dir` prefixes were never purged — 4484 dirs / 168 MB of `/tmp` litter | ✅ **fixed** 2026-08-05 |
| KI-29 | node/observe tests orphan `brood` children — one found alive **9 days** later, ~15% CPU each | ✅ **fixed** 2026-08-05 |
| KI-28 | `clean_peer_exit_fires_nodedown_promptly` failed once, then passed on retry; output not captured | ⚠️ **watching** (seen once 2026-08-05) |
| KI-27 | node tests drew their port from the OS **ephemeral** range, so an unrelated process could take it | ✅ **fixed** 2026-08-05 |
| KI-25 | five JIT/VM suites cannot be re-run in one image (`--repeat-until-failure` fails on iteration 2) | ✅ **fixed** 2026-08-04 |
| KI-24 | `eval`'d code cannot forward-reference a name a later `eval` defines (regression, 97d63eda) | ✅ **fixed** 2026-08-01 |
| KI-23 | the KI-22 lost-update shape also exists in ~10 std-module registries | ✅ **fixed** 2026-08-02 |
| KI-22 | concurrent registration lost ~40% of registrations (15 prelude registries) | ✅ fixed 2026-08-01 |
| KI-21 | `nest run --for` / `--watch` generated a legacy `~p` pin — failed on any file | ✅ fixed 2026-07-30 |
| KI-20 | a JIT fast link ran the callee against the *caller's* IC block (cold cache) | ✅ fixed 2026-07-30 |
| KI-19 | VM resolved a call's free-global head *after* its arguments | ✅ fixed 2026-07-30 |
| KI-18 | a JIT deopt could re-run a `table-put` (effect duplication) | ✅ fixed 2026-07-30 |
| KI-17 | `nest check` validated qualified names against its load set, not per-file reachability | ✅ fixed 2026-07-30 |
| KI-16 | the LSP still matched the retired `defprotocol`/`defimpl` | ✅ fixed 2026-07-27 |
| KI-15 | `impl` silently misregistered a **bare** record id | ✅ fixed 2026-07-27 |
| KI-14 | RUNTIME collector re-walked a deep process's whole root stack every safepoint | ✅ fixed 2026-07-27 |
| KI-13 | cross-module return-type inference blew up exponentially in branch count (checker hang) | ✅ fixed 2026-07-27 |
| KI-12 | a frozen prelude global's inner handle resolved to the wrong object | ✅ fixed 2026-07-26 |
| KI-11 | JIT tail-chain recursion escaped the native-depth cap | ✅ fixed 2026-07-26 |
| KI-10 | `receive` compile cliff at the 13th arm | ☑️ no longer reproduces (2026-07-25) |
| KI-9 | arity error from a closure shipped in a `spawn` body | ☑️ transient build artifact; not in committed code |
| KI-8 | RUNTIME form-position table stranded by compaction | ✅ fixed 2026-07-03 |
| KI-7 | declared `(sig …)` type-expressions corrupted by RUNTIME compaction | ✅ fixed 2026-07-03 |
| KI-6 | `%isolate` snapshot/restore not RUNTIME-compaction-safe | ✅ fixed 2026-07-03 |
| KI-5 | `nest test` OOMs — shared RUNTIME region accumulates every test file's code | ✅ fixed 2026-07-03 |
| KI-4 | bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote | ✅ fixed 2026-06-15 |
| KI-3 | RUNTIME compaction strands live VM / tree-walker constants | ✅ fixed 2026-06-01 |
| KI-2 | `nest test` flaky / hangs when parallel tests share heavy global lookups | ✅ fixed 2026-05-29 |
| KI-1 | multi-thread scheduler race: green processes can't resolve globals | ✅ fixed 2026-05-29 |

**No open issues; two watch items (KI-28, KI-36 — both single, unreproducible dist failures seen under load).** No open bug in the language, runtime or toolchain
itself. Every KI above is fixed, incidentally fixed, or a non-reproducing transient — each kept as
a record with its regression test, so a recurrence is recognizable.

---

## KI-36 — a single `reconnect_watcher_heals_a_fallen_link` failure · **watching, seen once 2026-08-07**

**Seen once**, in the full `make test` that gated the dependency-imaging change: `TRY 1 FAIL
[22.567s]`, passed on nextest's retry, suite otherwise 970/970. The box was **not idle** — a
4000-module image build (≈2.9 GB RSS) was running beside the suite, which is the same condition
the 2026-08-06 devlog records as producing a false `division by zero` failure in
`brood_suite_passes`.

**Not reproduced: 0 failures in 25 idle runs** (retries off, one at a time) and **0 in 10 runs
under synthetic load** (six CPU burners plus repeated cold image builds). That second number is
weak evidence, and worth saying so: those runs took 1.6–1.9 s each against 2.58 s idle, i.e. the
load never actually reached the test's path.

**What the 22.6 s says.** The test has three liveness deadlines — nodedown 15 s, pong 20 s,
nodeup 45 s. A 22.6 s failure can only be the **nodedown** one (15 s plus node startup), not the
nodeup deadline, which would have taken at least 45 s. That branch is A failing to observe
`[:nodedown]` after B1 exits *cleanly*, where the socket EOF should fire it immediately and the
heartbeat path (`DOWN_AFTER` 6 s, 2 s ticks) is only the backstop — so 15 s is roughly 2× the
worst case, and exceeding it means a real stall, not a marginal deadline.

**Diagnostic to arm next time:** the failing run's stdout was lost because the suite output was
piped through a `grep` for summary lines. The test prints exactly which branch fired
(`TIMEOUT-no-nodedown` / `NODEDOWN-OK` / `NOCONNECTION-OK` / …) precisely so this question is
answerable after the fact — capture the full nextest output on any suite run that is expected to
be evidence.

**A clean re-run is green:** 970/970 with no flaky row, on an idle box, same commit. So the
sighting stands alone, exactly as KI-28's does.

**Related:** KI-28 is the same shape in the same suite — a single unexplained dist failure that
passed on retry and has never recurred. Two independent one-off failures in the dist tests, both
under load, is worth correlating if a third appears; neither reproduces on demand today.

---

## KI-35 — `*method-from*` was never imaged · **fixed 2026-08-07**

**Filed separately from KI-34 on purpose**, for the reason KI-30 was filed separately from
KI-29: found while fixing another issue, and a bug recorded as an aside inside a fixed one is
invisible. This is also the *third* recurrence of one shape — a registry missing from the
startup image's hand-maintained list — after `declared_sigs` and the seven ability/multimethod
registries of 2026-08-07 morning.

**The defect.** `register-method` records `[multimethod key] → registering ns` in
`*method-from*` and warns when a *different* module re-registers the same key
("redefined from … by … (last wins)"). The image never carried that registry, so an imaged
start began with it empty: `has-prev` is false for every key, and the conflict warning stops
being emitted. Nothing crashes and dispatch still works — the last registration simply wins,
silently, which is exactly the class this list keeps producing.

**The fix is the mechanism, not the entry.** The set is derived now rather than named —
`%registry-update!`/`%registry-cas!` are the only ways a registry is written, so the kernel
records the names they write and `(%registry-names)` reports them (see KI-34 and ADR-218).
A registry added later is carried without anyone remembering to. What remains in
`std/tool/project.blsp` is an *exclusion* list, where a forgotten entry costs a redundant load
rather than a wrong answer.

**Guard:** `crates/nest/tests/startup_image.rs::an_imaged_start_keeps_what_loading_registered`
runs a project twice and asserts multimethod dispatch, ability dispatch, record identity,
method provenance and module docs all survive the imaged start.

---

## KI-34 — the startup image was written every cold start and never read from · **fixed 2026-08-07**

**Found 2026-08-07** while auditing KI-35's neighbourhood. ADR-218's per-module lazy
materialisation had not worked since it shipped (118f745a): `nest run` on a project restored
the image's root section and then **loaded every module from source anyway**. There is no error
and no wrong answer — the observable behaviour of an imaged start was identical to a cold one,
because it *was* a cold one. Only the benefit was missing.

**Two independent defects, either one sufficient to disable it.**

1. **The installer wrote a module-qualified global.** `project-install-image` ran
   `(def *image-sections* …)` from inside module `project`, binding
   `project/*image-sections*`, while `require-force` — root code — read the empty root global.
   The prelude documents this exact rule four lines above the global's own `def`, and ships
   `set-load-path!` as the root setter for the same reason; `set-image-source!` is now its
   sibling and the installer goes through it.
2. **`require-force` tested the branches in the wrong order.** The image branch sat after the
   ADR-070 package-module branch, and a project roots its OWN modules — so
   `*package-module-files*` holds `demo/shapes` exactly as it holds a dependency's `foo/b`, and
   the package branch matched first for every module of every named project. The image branch
   now comes first: an image is only installed after its fingerprint matched the sources that
   branch would re-evaluate, and a module with no section still falls through to it.

**Why the existing tests missed it.** `startup_image_test.blsp` and `image_test.blsp` drive the
`%image-*` primitives directly — they hand in a section list and read it back, never routing
through `require`. The defect lived entirely in the seam between the primitives and the loader,
which no unit of either side can see.

**How to tell it is working** (the only reliable signal): a `println` at a module's top level is
evaluated by a source load and absent from an imaged one. A second run of an unchanged project
must not print it, and `BROOD_IMAGE_TRACE=1` should show one `[image-section]` line per module
the entry point reaches, not just the root section.

**Guard:** `crates/nest/tests/startup_image.rs` — `a_second_run_loads_modules_from_the_image_not_from_source`
(plus `an_edited_source_file_invalidates_the_image`, so the fix cannot be "serve a stale image").

**Follow-up shipped the same day:** a dependency's modules are imaged too, with their files
added to the staleness key (they live outside `:source-paths`, so nothing else could invalidate
them) — see the ADR and `a_path_dependency_is_imaged_and_its_edits_invalidate`.

**Re-measured 2026-08-07, and the two halves of ADR-218 fared very differently.** Same fixture
(4 002 generated modules), same box, release binaries from `90099993` (pre-fix) and `34770be4`:

| row | pre-fix | post-fix | |
|---|---|---|---|
| lazy `nest run` (entry reaches 2 of N) | 0.37 s / 112 MB | 0.36 s / 102 MB | unchanged |
| eager materialise-all | 8.55 s / 674 MB | **1.34 s / 453 MB** | **6.4×**, −33% RSS |
| ⤷ materialise time alone | 7 012 ms | **959 ms** | **7.3×** |

So the **lazy row was never broken in a way a stopwatch could see**: an entry point reaching two
modules pays about the same to source-load two files as to materialise two sections, which is why
ADR-218's 1.30 s was a real number measuring the wrong mechanism (a 16 302-module re-run of that
row lands at 1.57 s / 243 MB against its 1.30 s / 219 MB, on a denser fixture). The row that was
genuinely broken is the **eager** one — `project-materialize-all` → `require-one` → the source
branch — so `nest test`, `nest check` and the LSP re-evaluated the whole project on every start.

**A trap worth keeping:** the eager path *reported* "materialised 4002 of 4003 sections" in both
arms. The count is sections walked, not sections served from the image, so the instrument looked
healthy throughout. Only the wall clock and a top-level `println` in a module told the truth.

---

## KI-33 — fully consuming a stream leaked its producer process · **fixed 2026-08-07**

**Found while fixing KI-32, 2026-08-07.** A `std/stream.blsp` stream is a process; an
exhausted one used to answer `:stream-done` *idempotently* by looping in `stream-done-loop`
— so once a stream was fully consumed, its producer process stayed **parked forever** instead
of exiting. `stream-empty`, `stream-singleton`, `stream-drop` (when it drops past the end),
`stream-chunk` (partial last chunk) and `stream-lines` (final buffer flush) all ended there.
Every *other* source (`stream-list-loop`, `stream-fn-loop`) and every transformer already
exited on exhaustion, so this was an inconsistency: some pipelines self-cleaned, some leaked
one (or more) processes per stream. Harmless for a one-shot, but a long-lived program that
builds many streams leaked processes without bound — surfaced as `memory limit exceeded`
under a stream-hammering repro.

**Fixed** by making `stream-done-loop` (and `stream-err-loop`) answer once and then **exit**,
matching every other loop. Safe because a well-behaved consumer stops at the first `:done`
(the idempotence was never relied on) and no transformer re-polls a done upstream — each
forwards `:done`/`:stream-err` and exits. Terminal consumers therefore need no `stream-close`
call, and a fully-consumed multi-stage pipeline tears itself down completely.

Regression test: `tests/stream_test.blsp`, "streams do not leak producer processes" — consumes
150 batches of the four leaking shapes and asserts no producers remain parked under this
process (a `process-info :parent`/`:status` count, cross-checked to report 5 for 5
deliberately-parked children). **Verified by sabotage** — restoring the recursion fails the
test 3/3.

---

## KI-32 — a selective `receive` corrupted a skipped local message to `nil` · **fixed 2026-08-06**

**Found by the green-tree flake battery, 2026-08-06** — the in-language suite's
`stream-drop › drop more than available` timed out at 600s once (masked by a nextest
retry, so the battery's own fail-count read 0). Not a stream bug: an amplified repro
(`stream-drop` pipelines hammered concurrently) reproduced an intermittent **hang** —
~20% at 200 pipelines, ~80% at 800, and **100% single-worker** (`BROOD_J=1`), which ruled
out a multi-core data race and pointed at a cooperative-scheduling logic bug. A gdb
snapshot of the wedge showed the root process blocked in `receive` while the worker slept
with an empty run queue; a watchdog dump named the culprit: one process parked over a
single queued message whose value had become **`nil`**.

The bug was in the selective-receive scan (`scan_mailbox`, `process/mailbox.rs`). A message
delivered by the **L1 fast path** — the sender copies it straight into a *parked* receiver's
heap, a `Payload::Local` whose value lives in a `msg_roots` slot — was read during the
optimistic single-lock scan with **`msg_root_take`**, which *tombstones the slot for reuse*.
On a **non-match**, the same envelope (still holding that slot index) was re-inserted into
the queue via `reinsert_candidate`; the next scan `msg_root_peek`-ed the now-empty slot and
read **`nil`** (or, worse, a later message that reused the slot). A `[:next pid]` request
sitting in a stream stage's mailbox while it waited for its upstream reply was thus corrupted
to `nil` and could never match again — the stage parked forever, and the whole pull-stream
pipeline deadlocked. Intermittent because it needed the message to arrive via L1 (receiver
parked) *and* be the optimistically-popped first candidate of a non-matching scan; worse
single-threaded because that interleaving is more likely; engine-independent because the
mailbox scan is shared by the VM and the tree-walker; unhelped by every scheduler/message
opt-out lever because none of them touch the take/reinsert.

**Fixed** by **peeking, not taking**, in the optimistic scan (so a re-queued candidate keeps
its slot intact) and freeing the slot (`msg_root_take`) only on the **consume** path, where
the message actually leaves the queue. This also closes a latent slot leak on the
non-optimistic match path, which peeked but never freed. Wire-format messages were always
immune (their value lives in the envelope, not a slot).

Regression test: `tests/concurrency_test.blsp`, "selective receive: a skipped local message
keeps its value" — a receiver is driven to `:waiting` (via `process-info`) so the send takes
the L1 path, then a non-matching `[:data 42]` is skipped and later matched; a companion
hammers the shape across 40 processes and checks the value round-trips. **Verified by
sabotage** — on the pre-fix binary the test hangs 5/5 (600s → timeout); on the fix it passes
0/10, and the amplified repro drops from 80%/100% hang to 0.

---

## KI-31 — a foreign-ecosystem version range silently compiled to its first term · **fixed 2026-08-06**

**Found by a bug hunt against `std/version.blsp`, 2026-08-06** — not by a failing test, and not
by the fuzzers: no generator covers the version module.

`version-satisfies?`/`version-compile` split a constraint into terms on **commas**. A range
written in npm/cargo style, with terms separated by *spaces*, therefore arrived as one term:
`version-split-constraint` peeled the leading `>=` and handed `"1.0.0 <2.0.0"` to
`version-core`, which is **deliberately lenient** ("a missing or non-numeric segment reads as
0" — the property that makes `"1.x"` and the short `">= 0.3"` work). It parsed that as
`(1 0 0 0 0)`, i.e. `>=1.0.0.0.0`. The upper bound vanished without a word.

The consequence is the dangerous direction for a package manager:

```
(version-satisfies? "3.0.0" ">=1.0.0 <2.0.0")   ; => true, before the fix
```

A user pinning a dependency to `>=1.0.0 <2.0.0` got `>=1.0.0`, so the resolver could select a
breaking major. It also failed the other way — `"^1.0.0 || ^2.0.0"` silently dropped the second
alternative, so `2.5.0` did *not* satisfy it — and `"1.0.0 - 2.0.0"` (npm hyphen) became
`=1.0.0` with a garbage prerelease of `(" 2" "0" "0")`.

**Fixed** by rejecting a version part containing whitespace or a constraint operator
(`< > = | ^ ~`) — the same rule `version-split-constraint` already applied to the *operator*
("an operator with no meaning has to say so"), extended to the version beside it. `-` and `+`
are deliberately not rejected: they are legal prerelease/build punctuation.

**The leniency it does not touch, on purpose.** `">= 1.0.0"` (space after the operator),
`">= 1.2"` (short), `"1.x"` and `"1.0.0+build"` all still compile — that leniency is why
`version-core` exists in its current form. One case stays lenient and is *not* fixed:
`">=1.0.0extra"` still reads as `>=1.0.0`, because a segment that begins with digits and
continues with letters is indistinguishable from the `"1.x"` form the module intends to
accept. Tightening that means deciding whether `"1.x"` should survive at all — a design
question, not a bug fix.

Regression test: `tests/version_test.blsp`, "a range from another ecosystem raises instead of
silently widening", plus a companion asserting the intended leniency survives. **Verified by
sabotage** — with the check disabled the first fails and the second does not, which is the only
version of that test worth having.

---

## KI-30 — seven `temp-dir` prefixes were never purged · **fixed 2026-08-05**

**Filed as its own entry deliberately.** Found while fixing KI-29, and KI-29's lesson is that an
adjacent finding recorded inside another entry is invisible. It is a *different* bug: litter on
disk, not a process burning CPU and holding a port.

**Measured.** 4622 `/tmp/brood-*` directories, **168 MB**, on an ordinary dev box.

**The mechanism already existed — the coverage did not.** My filed fix direction (a
`with-temp-dir` macro, and a decision about whether a failing test keeps its directory) was
wrong, because `purge-stale-temp` already implements the convention and documents it: name
fixtures with a unique prefix, and at file **load** drop the previous run's leftovers, which
bounds `/tmp` to one run and recovers from a crashed run. Nine prefixes did this. Seven did not,
and the census is a clean 1:1 with no inference required:

| prefix | dirs | purged? |
|---|---|---|
| `brood-feat-` | 1392 | ❌ |
| `brood-reload-` | 926 | ❌ |
| `brood-walk-` · `brood-skip-` · `brood-p2-` | 464 each | ❌ |
| `brood-ambient-` · `brood-ambient2-` | 387 each | ❌ |
| `brood-pkg-` | 42 | ✅ |
| `brood-file-`, `brood-slurpb-`, `brood-dup-`, `brood-manifest-`, `brood-fmt-*`, `brood-buf-` | ≤20 each | ✅ |

Every unpurged prefix is in the hundreds; every purged one is one run's worth. 4484 of the 4622
directories (97%) come from the seven missing lines.

**Fix.** The seven missing `(purge-stale-temp …)` calls, in `project_test.blsp` (4),
`reload_watch_test.blsp` (1) and `syntax_finalization_test.blsp` (2). Verified by running the
whole suite three times: **128 directories after each run** (110 of them the suite's, 3.7 MB),
flat, where before each run added ~110 permanently.

**Covered by** `tests/temp_purge_coverage_test.blsp`, which scans each test file's own source and
fails if a prefix passed to `temp-dir` is never passed to `purge-stale-temp` — reporting
`[file prefix]` pairs, so a failure names the file to edit and the line to add. A purge covers a
prefix it is a `starts-with?` prefix of, since that is how the purge itself matches
(`brood-file-` legitimately covers `brood-file-swap-`). **Verified by sabotage**: deleting one
purge line fails it with `(["reload_watch_test.blsp" "brood-reload-"] …)`. It carries a second
test asserting the scanner still finds ≥20 uses, so a renamed primitive or a reformat that breaks
the `(temp-dir "` spelling cannot make the first test pass vacuously forever.

**Why a source scan rather than a `/tmp` check:** a `/tmp` check could only fail *after* litter
exists, would depend on what earlier runs left behind, and would not reproduce on a clean machine.
The property is static and local — a file's own text either has the line or it does not.

**Not a correctness risk** — nothing reads a stale dir (each name is freshly randomised), and the
purge runs at load, before any fixture exists. It was disk, and noise when reading `/tmp`.

## KI-29 — the node/observe tests orphan `brood` children · **fixed 2026-08-05**

**Why this became its own entry.** It was written down as an "adjacent finding" inside KI-14, which
is *fixed* — so it read as closed, was counted as closed, and sat for nine days. A live bug filed
under a fixed one is invisible. (KI-30 is filed separately for exactly this reason.)

**Evidence, 2026-08-05.** Three stray `brood` nodes were alive on this box at once:

| child | age | CPU |
|---|---|---|
| `/tmp/brood-observe-*/target.blsp` | **9 d 14 h** | 4.0% |
| `/tmp/brood-dist-reconnect-*/b1.blsp` | 1 h 13 m | 15.3% |
| `/tmp/brood-dist-nodedown-*/quitter.blsp` | 32 m | 15.6% |

Together ~35% of a core, indefinitely, on a machine whose benchmark numbers are measured pinned to
one core. Each is also a **live node**, listening, with the same `secret-test-cookie-16+` and the
same `:a`/`:b` names every dist test uses — which is why it was KI-28's leading suspect.

**The cause: a test binary that is killed rather than allowed to finish** — nextest fail-fast, a
`timeout`, a `^C`. `std::process::Child` has no drop-kill, so every `brood` child the binary had
spawned is reparented and runs forever. Nothing fails when this happens, which is why it went
unnoticed: a leak leaves no red test behind.

**Fixed** in `crates/cli/tests/support/mod.rs` with `BroodChild`, a guard returned by
`spawn_brood`/`spawn_brood_env` in place of a bare `Child`. Two *independent* nets, because neither
covers the other's case:

1. **`Drop`** kills and reaps whatever is still running — for the test that panics between the
   spawn and its `kill`, or returns early past it.
2. **`PR_SET_PDEATHSIG(SIGKILL)`** in `pre_exec`, so the *kernel* kills the child when the thread
   that spawned it terminates, however it terminates. This is the net that fixes the filed bug: a
   SIGKILLed test binary runs no destructors, so net 1 is worth nothing there. It closes its own
   race too — if the parent died in the window between the fork and the `prctl`, the signal was
   already missed, so the child compares `getppid()` against the pid captured before the fork and
   `_exit`s if it changed.

Net 2 is exposed on its own as `dies_with_parent(&mut Command)` and applied to the four test files
that merely `.output()` a one-shot `brood`/bundled-app run as well (`error_format_parity`,
`checker_cross_module_ability`, `std_attribution`, `release_bundle`). Those cannot orphan on the
happy path — `output()` blocks until the child exits — but a program that *hangs* while the binary
is killed is the same leak, and this bug's lesson is that nothing reports it. The invariant is now
uniform: every child a `cli` test starts dies with the test.

**Two things the original entry got wrong**, both worth keeping:

- **Cause 2 — "the observe/attach path leaks even when its test completes normally" — has no
  mechanism, and is withdrawn.** It was inferred from the 9-day-old child being "from an ordinary
  run", which was itself inferred from its age; age is not provenance. Read the test: all three
  harnesses kill the target *before* asserting, so a run that completes cannot leak, and the only
  non-killing exits are panics — i.e. cause 1 again. Verified: `cargo nextest run -p cli` is 46/46
  with zero strays. (The fix covers this either way — PDEATHSIG does not care why.)
- **The filed fix direction, a process group, is the wrong lever for cause 1** and was not taken.
  Moving the child into its *own* group **removes** it from any group an outer tool might kill, and
  nothing runs our group-kill when we are the one being SIGKILLed. A group buys only grandchildren,
  which no test here creates (these programs are green processes, no `run-process`), and it costs a
  pid-recycling hazard: `killpg` on a reaped leader's recycled pgid can signal an unrelated group.
  `Child::kill` cannot — std caches the exit status, so a kill after a `wait` is a no-op rather
  than a signal to a stranger's pid.

**Covered by** `crates/cli/tests/child_cleanup.rs`. "Nothing leaked" is not observable from inside
a passing test, so each test drives **one** net with the other defeated and asserts the child
actually died: the `Drop` test spawns on the test's own thread (where the parent-death signal
cannot have fired yet), and the PDEATHSIG test `mem::forget`s the guard so no destructor runs.
Both were **verified by sabotage** — breaking each mechanism fails its own test at the 10 s
deadline and leaves the other passing, and the PDEATHSIG sabotage reproduced the original leak
live (a child outliving its dead test binary).

**Also verified against the filed scenario itself**, since the unit tests stand in for it rather
than perform it: `cargo nextest run -p cli --test distribution` was started, a test binary holding a
live `brood` child was **SIGKILLed** mid-run, and three seconds later `pgrep -af 'brood /tmp/brood-'`
was empty. Do this with a **bracketed** pattern (`'deps/[d]istribution-'`) — the unbracketed form
matches the shell running it, which is the trap `CLAUDE.md` records, and it cost a run here by
SIGKILLing the very shell doing the killing.

**Check for a recurrence with** `pgrep -af 'brood /tmp/brood-'` — still the right instrument, since
the class of bug is one that no test failure reports.

## KI-28 — a single unexplained `nodedown` flake · **watching, seen once 2026-08-05**

**Not a diagnosis — a record, so a second occurrence is recognised as the second.** In the full
`make test` that verified the KI-27 fix, `cli::distribution clean_peer_exit_fires_nodedown_promptly`
failed try 1 and passed try 2 (nextest reported `FLAKY 2/2 [0.631s]`).

**How long the failure took is unknown.** That 0.631 s is the duration of the *passing retry* —
nextest reports the final attempt. An earlier version of this entry read it as the failure's
duration and concluded node A had exited early rather than hitting its 5 s `[:nodedown]` guard;
that inference had nothing under it and is withdrawn. Both shapes are still open: a failed
`connect`, or a nodedown that genuinely did not arrive within 5 s under load.

**What the failure text said: nothing, and that is my fault.** I had piped that run through
`tail -25`, which discarded the assertion output. The test now prints **B's stderr** alongside A's
on failure, so the next occurrence explains itself.

**What is known.** 0/40 solo; 33/33 × 3 for the whole dist file under nextest (the config that
failed); and it did **not** recur in the next full `make test` (956/956, no flaky). It is *not* the
KI-27 mechanism — under nextest this test's process asks for two ports and cannot wrap its slice,
and concurrent test processes have near-consecutive pids and therefore disjoint slices.

**Its one live suspect is now gone, which matters for reading a recurrence.** Orphaned test children
(KI-29) were real and confirmed 2026-08-05: three stray `brood` nodes alive at once, one for **9
days**, together burning ~35% of a core. A leaked node still *listening* on a port, with the same
`secret-test-cookie-16+` and the same `:a`/`:b` names every dist test uses, is exactly the kind of
thing that makes a node test fail once in a hundred runs. **KI-29 is fixed as of 2026-08-05**, so a
run since then starts from a box with no stray nodes on it — meaning a recurrence *after* that date
can no longer be explained away by the leak, and is a stronger signal than the original sighting.
Still worth confirming with `pgrep -af 'brood /tmp/brood-'` before chasing anything subtler.

**If it recurs**, the printed B stderr should immediately separate "B never bound its port" from
"A could not connect to a listening B"; a `connect` failure would then be a real runtime question
rather than a harness one.

**Dedicated hunt 2026-08-07: 0 failures in 260 runs, retries OFF.** With no stray `brood` nodes
on the box (confirmed `pgrep -af 'brood /tmp/brood-'` clean), the whole dist file ran 60× under
nextest (`--retries 0 --no-fail-fast`, the exact config that flaked) and the named test ran 200×
alone — zero flakes, zero aborts. That is now the standing record: KI-28 **genuinely does not
reproduce on demand**, so it stays a watch item rather than an open bug, with the B-stderr
diagnostic armed. A recurrence after this date — on a box with the KI-29 leak fixed — is a real
signal worth chasing; until then there is nothing under it to fix.

## KI-27 — node tests drew their port from the OS ephemeral range · **fixed 2026-08-05**

**Symptom.** In a full `make test`, `cli::distribution reconnect_watcher_heals_a_fallen_link`
failed twice (nextest retried once) after **20.06 s** — its `(after 20000 …)` guard on
`[:nodeup]`, so the watcher never saw node B come back. Fast and reliable alone: **7/7 solo** at
~1.3 s, **16/16** as 16 concurrent copies of the same binary, and 3/3 for the whole dist binary
even under 12 CPU hogs. Only a full suite reproduced it.

**Cause.** `free_port()` in the three node-test harnesses picked a port with
`TcpListener::bind("127.0.0.1:0")` and dropped the listener. That asks the kernel for a port from
the **ephemeral range** — 32768–60999 here (`/proc/sys/net/ipv4/ip_local_port_range`) — which is
the same range every outbound connection on the box is assigned from, and then releases it. So a
port a test node is about to bind can be handed to an unrelated process's *client* socket in the
gap, and the node's bind fails with `EADDRINUSE`.

Measured, not inferred: of 4000 client sockets opened by an unrelated process, **4000 (100%)**
landed on a port the old `free_port()` could have handed a node, and **0** in the band the fix
uses. `reconnect_watcher_heals_a_fallen_link` is the most exposed test in the file because it needs
one port to stay free across B1's whole life, B1's exit, the 400 ms gap *and* B2's rebind; every
other test needs a single bind. And the pre-existing mitigation — a `PORTS` mutex around
bind→spawn — is per-process, so under `cargo-nextest` (one process per test, which is what
`make test` uses) it does not exist between the tests that actually run concurrently.

**Fix.** `crates/cli/tests/support/mod.rs` (new — the three harnesses had their own copies of these
helpers, which is how a fix in one file could miss two). `free_port()` now allocates from a fixed
band **below** the ephemeral floor (12000..32768), where the kernel never auto-assigns, sliced by
pid so concurrent test processes start in different places, and probes bindability before
returning. Additionally `reconnect_watcher_heals_a_fallen_link` now calls `wait_until_listening`
on B2 instead of assuming it came up, and prints B2's stderr on failure — the test previously
could not distinguish "the watcher failed to heal" (the thing under test) from "B never came
back", which is why this presented as a mysterious 20 s timeout. Its `[:nodeup]`/pong deadlines
were raised (60 s / 30 s): they are liveness ceilings on a saturated machine, not latency
assertions, and cost nothing when healthy (the test still runs in 1.3 s).

**Ruled out along the way.** `SO_REUSEADDR` is absent from all three `TcpListener::bind` sites
(`net.rs:1208`, `net.rs:1528`, `dist.rs:859`) — the obvious suspect for a failed rebind, and
**tested directly: not the cause** (immediate rebind after a linked peer exits succeeds 3/3).
That is consistent with the real cause: the port was lost to another process, not to `TIME_WAIT`.

## KI-25 — five JIT/VM suites cannot be re-run in one image · **fixed 2026-08-04**

**Symptom.** Five suites pass on the first run and fail on the second *within the same
image* — that is, under `--repeat-until-failure`, at every seed tried (0 and 5), which rules
out ordering:

| suite | shape |
|---|---|
| `jit_self_rebind_test` | 1 of 2 fails on iteration 2 |
| `jit_shared_spawn_test` | 1 of 4 |
| `pid_identity_test` | its single test — and it is ALREADY `:isolated` |
| `vm_call_head_order_test` | 2 of 3 |
| `vm_selfcall_reload_test` | 1 of 3 |

Found by a sweep of every suite in both repos (3 iterations × 2 seeds). `bedit` came
back completely clean; these five were the only hits, and they reproduce on a working tree
with the day's changes stashed, so they predate them.

**Why it matters more than "who re-runs a suite twice".** `--repeat-until-failure` is *the*
tool for finding flakes, and `nest test --failed` re-runs in one image too. While these
five fail on a second iteration, the tool cannot be used across brood's suite — a real flake
somewhere else is invisible behind them. (That is how it was found: hunting flakes elsewhere.)

**What is known.** These are hot-reload / self-call / tiering tests: they redefine globals
and assert on how the VM or JIT re-links afterwards, so the natural theory is "iteration 2
starts from iteration 1's redefined state". That explains four of them, and `:isolated`
(which runs a unit alone against the clean post-load baseline and rolls back its `def`s) is
the ordinary fix.

**`pid_identity_test` is explained too, and NOT by a JIT cache** (correction 2026-08-04 — the
original entry guessed "a JIT tier election, an IC block, a pid counter, or an arm-keyed cache"
and reasoned from there that the case was informative about JIT internals; it is not). Running
the recipe below prints the cause outright:

```
uncaught error: … :message node-start: this runtime is already a node (node-start called twice)
```

The test calls `node-start`, which is **deliberately one-shot per runtime** (`dist.rs`: a second
listener would need a second port, so the second call is an error by design). `:isolated` rolls
back `def`s; it cannot roll back the runtime's node state, and no test-level isolation can. So
this is not "something survives an isolated re-run" in the mysterious sense — it is a test whose
precondition (`(node-name)` is `:nonode`) is consumed by running it once.

**Fixed 2026-08-04, and it was small once the premise was right.** Two different fixes, as the
diagnosis implies:

- **The four rebinding suites** (`jit_self_rebind`, `jit_shared_spawn`, `vm_call_head_order`,
  `vm_selfcall_reload`) are marked **`:isolated`**, so `%isolate` rolls their `def`s back to the
  post-load baseline and iteration 2 starts where iteration 1 did. `jit_shared_spawn` moved from
  `:serial` to `:isolated` — it already ran alone; what it lacked was the rollback.
- **`pid_identity_test`** now calls `node-start` only when `(node-name)` is `:nonode`. Nothing at
  the test level can undo a started node, so the re-run keeps the equality / hashing /
  receive-pattern assertions and skips re-taking the one-shot transition. Every ordinary
  `nest test` is a fresh image, so the real nonode→node case still runs there.

**Verified**: each of the five clean over 3 iterations at seeds 0 and 5, and — the point of the
issue — the **whole in-language suite now survives a re-run in one image**: `nest test
--repeat-until-failure 2` gives 4390/4390 twice. The failure detector was checked against the
pre-fix file first, so a green result means something. Reproduce in one command:

```bash
nest test tests/pid_identity_test.blsp --repeat-until-failure 3 --seed 0
```

## KI-24 — an `eval`'d definition cannot forward-reference another `eval`'d name · **fixed 2026-08-01**

**Symptom.** Two definitions made by `eval` from inside a namespace, where the first
references the second:

```
(defmodule m)
(eval '(defn r (n) (s n)))
(eval '(defn s (n) (+ n 1)))
(r 41)          ; → unbound symbol: s
```

The hint said it all: ``s` is defined as `m/s``. The *definition* side qualified
correctly; only the bare *reference* failed to.

**The REPL had it too** — it is simply typing a `defmodule` and then two mutually
recursive `defn`s, since each REPL input is its own compile unit:

```
(defmodule rtest)
(defn ra (n) (rb n))
(defn rb (n) (+ n 1))
(ra 41)          ; → unbound symbol: rb
```

**Order-dependent; ordinary code unaffected** — `scripts/fuzz/stress/eval_forward_ref.blsp`:

| shape | before | after |
|---|---|---|
| normal mutual recursion (forward ref) | ✅ `:ok` | ✅ |
| eval'd, target defined **first** | ✅ 42 | ✅ |
| eval'd, target defined **second** | ❌ `unbound symbol: s` | ✅ 42 |

**Confirmed a regression** rather than long-standing: rebuilding `builtins/system.rs` at
`97d63eda^` returned 42 for the failing row.

**Cause.** `97d63eda` (a good change — it took `eval` off the ~14× tree-walker) swapped
`macroexpand_all` for `macros::compile`, which adds the **resolve** pass. Resolve qualifies
a bare name only on *positive evidence* that the namespace owns it: the name is already a
`ns/name` global, or it is in `ns_known_names` — the def heads a **file loader pre-scans
before compiling any form**. That pre-scan is what makes a forward reference work inside a
file. `eval` and the inheriting `eval-string` (REPL, inline) compile **one form at a time**
and cannot scan the future, so the reference was left bare. A survey of every
`macros::compile` call site confirmed those two were the only ones without a pre-scan.

Note the mirror image: *before* the resolve pass, an eval'd `defn` inside a module defined
a bare **root** global — module code leaking into the global namespace. The old behaviour
"worked" only because reference and definition were consistently wrong.

**Fix.** `Heap::set_ns_assume_own` — a compile-context flag, alongside `compile_ns` /
`ns_known_names`, set by the two call sites that have no pre-scan behind them. With it on,
the resolver's *last resort* flips: a bare name bound at root/prelude still falls through
(so `+`/`map`/`count` keep working) and a `(:use …)` import still wins (checked first), but
a name bound **nowhere** is taken to be this namespace's — the conclusion the file pre-scan
would have reached. Deliberately not the resolver's default: over-qualifying a name that is
really a local is a *silent* miscompile, so the general rule stays evidence-only.

**The one trap, caught by the regression test.** Special forms and core-macro keywords —
`if`, `let`, `do`, `fn`, `cond` — are bound in **no** environment, so "unbound ⇒ ours"
rewrote `if` to `m/if` and every eval'd conditional died. `is_syntax_keyword` excludes
`builtins::SPECIAL_FORMS`. (Macro *calls* are already gone by resolve time —
`macroexpand_all` runs first — but a special-form head survives expansion by definition.)

**Covered by** `tests/eval_vm_test.blsp` ("eval resolves names against the enclosing
namespace"): the forward reference, mutual recursion across two evals in O(1) stack, the
definition landing in the namespace rather than root, and one guard test each for syntax
keywords, root/prelude names, and imports. Those evals run at **load** time on purpose —
`compile_ns` is only set while a file loads, so an `eval` inside a `test` body is at root
and never exercises the resolver at all.

**Two fixes were written for this, independently and on the same day.** The other one
(`13706580`) reverts `eval_builtin` to `macroexpand_all`, dropping the resolve pass rather
than feeding it the missing evidence. It fixes the reproducer, and its reasoning about the
pre-scan is the same as above. It was measured against this one and not kept, because
dropping resolve drops everything else resolve does for an eval'd form:

| | revert to `macroexpand_all` | `compile` + `ns_assume_own` |
|---|---|---|
| `eval_forward_ref.blsp` | ✅ `:ok` / 42 / 42 | ✅ `:ok` / 42 / 42 |
| REPL: `defmodule` + two mutually recursive `defn`s | ❌ `unbound symbol: rb` | ✅ 42 |
| eval'd `defn` in a module defines `mod/name` | ❌ leaks a bare **root** global | ✅ |
| `(:use …)` imports, `(:alias …)`, privacy, static-QQ in eval'd code | ❌ lost | ✅ |

The REPL row is the load-bearing one, and it is why that commit's "`eval-string` was never
affected" does not hold: each REPL input is its own compile unit, so `eval_string_inner`'s
*inheriting* path (`reset_ns = false`) has no pre-scan either — its own comment says it
"does neither" — and touching only `eval_builtin` leaves the REPL broken. Verified by
building both and running the same two inputs.

## KI-26 — a fast-link deopt guard re-read `inline_installed`, and its fallthrough re-ran · **fixed 2026-08-04**

**Severity: unknown — structurally reachable, never observed.** Recorded because the root cause
is the same anti-pattern behind two real ADR-210 bugs, and because the failure mode would be a
silently *repeated effect* rather than a crash.

**The shape.** In `jit_run_fast_link` (`eval/compile/jit_runtime.rs`), a deopt takes the
resume path only if `arm.active_nslots() == nslots`, where `nslots` is the frame size recorded
in the fast-link table and `active_nslots()` **re-reads `inline_installed`**. When the guard
declines, control falls through to `vm_apply` — a re-run from ip 0, which repeats any effect the
native had already journaled (a completed non-tail call, or a `table-put`).

**Why it looks reachable.** The inline swap in `jit_tier` deliberately does **not** bump the
global epoch — its own comment explains that a bump cascaded under `pfib` and cost ~2× — and it
invalidates only the *installing* process's fast links. Arms with a `share_key` are shared
across processes via `shared_closure_lookup`, so a peer can hold a link whose recorded `nslots`
predates the swap while `inline_installed` now reads true. Guard declines → re-run → duplicate.

**Why it is not fixed.** It could not be demonstrated. A purpose-built detector — fire whenever
the guard declines *while `jit_ckpt_resume` reports a live journal* — stayed silent across:

- the 4350-test in-language suite,
- `tests/jit_effect_once_test.blsp` (all six cases),
- `pfib` ×3, the parallel shared-arm workload the no-epoch-bump comment is about,
- a targeted 24-process race: one shared arm with a `table-put`, a spliceable leaf (so it gets
  an inlined upgrade), and an i64 overflow forcing a deopt on *every* call. All 24 effect
  counts came out exactly 4000, five runs in a row.

Most likely the per-call epoch check on the fast link re-validates the entry before the mismatch
can be observed. Not proven either way.

**Fixed** by extracting the check as a flag-free predicate, `jit_frame_shape_matches(arm,
frame_nslots)` = `frame_nslots == arm.nslots || frame_nslots == arm.inline_nslots`. That is a
strict superset of the flag form (`active_nslots()` returns exactly one of those two), so it only
*admits* more resumes — the effect-preserving direction — and every admitted resume is still
validated by `jit_ckpt_resume` (positive journal, in-bounds slot). A genuinely foreign arm is
still refused, which is the out-of-bounds protection the check exists for.

**How it was verified, given the runtime detector never fired.** The race could not be won, so
the behaviour difference was pinned at the unit level instead — which turned out to be the better
tool anyway, because it is deterministic. `ki26_frame_shape_check_is_independent_of_inline_installed`
asserts that for an arm with layouts (6, 9), *both* frames stay resumable in *both* flag states,
and that a foreign frame is refused in both; it **fails against the old flag form** (checked by
temporarily restoring it: "the small frame must stay resumable with inline_installed=true") and
passes with the fix. `ki26_shape_check_admits_everything_the_flag_form_did` sweeps four layout
pairs — including the degenerate `(4, 4)` an unjournalled derivation produces — × both flag
states × 40 frame sizes, asserting the new form never refuses what the flag form accepted, so the
fix cannot have traded one silent wrong-resume for another.

Gate re-run: 40/40 `tests/jit.rs` under GC_STRESS+GC_VERIFY, 946/946 Rust, 4350/4350 in-language
with `BROOD_NO_PARTIAL_LEAF` both ways, 4 fuzz generators × 4 engine configs, `--no-default-features`
clean.

**The lesson worth keeping.** The runtime detector was the right instinct and the wrong
instrument: it can only fire if you win a race. When a hazard is a *predicate* rather than a
timing window, extract the predicate and test it directly — a fix for a bug you cannot make fire
is a fix you cannot verify, but "make it fire" does not have to mean "reproduce it end to end."

## KI-23 — the KI-22 lost update also exists in std-module registries · **fixed 2026-08-02**

KI-22 fixed the **prelude's** registries by routing them through `%registry-update!`. The
same `(def *X* (assoc *X* …))` read-modify-write survived in registries that live in `std/`
modules. Measured directly, at 200 concurrent writers into one map: the plain `def` rebind
keeps **~125 of 402** entries (three runs: 128 / 122 / 125). The rest are silently gone.

It was already biting: `tests/repl_test.blsp`'s "re-registering a name replaces rather than
duplicates" failed **2 runs in 5** — two concurrent registrations each filtered the old list
and each appended, so the duplicate the API forbids survived. It is **32/32** now.

**The mechanism: compare-and-swap, not more ops.** `%registry-update!` names each shape it
supports as a kernel op (`:assoc`, `:cons-new`, …), which fits a registry whose update is one
map/list operation. Several of these are not like that — `face-set` merges into the *existing*
entry, `attach` strips an id from every bucket before consing onto one,
`register-repl-command` filters by name overlap and appends, `register-system-layer` is
append-if-absent. A Rust op per shape would have pushed policy into the kernel. Instead:

- `%registry-cas!` (`Heap::registry_cas`) — rebinds the global only if its current value still
  equals the expected one, under the same registry lock.
- `registry-swap!` / the `swap-registry!` macro (**prelude, Brood**) — retry around it, so the
  transform is an ordinary Brood function and only the read-decide-write is indivisible.

**The `defdyn` caveat dissolved.** The original note worried that `%registry-update!` writes at
`env_root(env)` and would bypass an active `binding`. Reading the evaluator settles it: `def`
computes `let root = heap.env_root(env)` and writes *there* too, and both spellings read
through the chain. The lock matches `def` exactly, so converting a `defdyn` registry (`*faces*`)
is semantics-preserving — an active `binding` shadows the root write identically either way.

**Converted** (13 sites): `debug/*traced-fns*`, `protocol/*protocols*`,
`telemetry/*telemetry-handlers*` (attach, detach, and the auto-detach of handlers that threw),
`telemetry/*telemetry-events*`, `*faces*` (`def-face` + `face-set`),
`editor/layers/*system-layers*`, `editor/layers/*type-layers*`,
`editor/layers/*auto-type-by-file*`, `repl/*repl-commands*`.

**Deliberately not converted: `std/tool/test.blsp`'s `*units*` / `*collected*` /
`*collecting*` — they are not racy.** Registration happens at *load* time and test files load
**sequentially**, each inside its own `%isolate` (`test.blsp`: "Files run sequentially (each
`%isolate` blocks to …)"). There is no concurrent writer to lose an update to. `*collecting*`
/`*collected*` are also not registries but a sequential accumulator protocol — `describe` sets
a flag, the body conses, the flag clears — which a CAS would not make correct anyway.

**Covered by** `tests/registry_test.blsp`: the mechanism with its own **control** (a plain
`def` rebind in the same test, asserted to lose entries, so a regression cannot hide behind a
test that would pass with the bug), a concurrent counter (proving each retry recomputes
against the value that won), and one concurrent-registration test per converted registry —
including the repl duplicate-name shape and `register-system-layer`'s idempotence.

## KI-22 — concurrent registration silently lost registrations · **found + fixed 2026-08-01**

**Symptom.** Intermittent failure of the whole in-language suite, ~1 run in 5, in three
different tests: `ability_test`'s "open extension" (`(esize [1 2 3])` returning `-1`, the
`:default` impl, right after its `:vector` impl was registered on the line above),
`modules_test`'s "provide records a feature idempotently" (its own feature missing from
`*features*` on the next line), and `private_test` (a module registered by a sibling test not
found). Because nextest retries, `make test` printed `929 passed` with only a `FLAKY` marker.

**Root cause: a lost update.** Every load-time registry is one global holding a whole map or
list, written as `(def *X* (assoc *X* …))` — read, compute, write, three separate steps. Two
processes registering at once each read the old value and each write their own successor; the
later write silently drops the earlier one. `scripts/fuzz/stress/registry_race.blsp` (N
processes, one **private** ability each, so nothing is a legitimate precedence contest)
measured **24/50, 88/200, 218/500 lost — about 40%.** Not a dispatch-cache bug; the cache was
never involved. **Fifteen registries** share the shape, so multimethod registration had the
identical bug to ability registration, unhit until now.

**Fix: `%registry-update!`** — one kernel primitive that performs the whole read-modify-write
inside a single call, under a per-runtime `registry_lock`. Atomic by construction: no CAS (so
no ABA question), no retry loop, no spinning, and no callback into Brood while a lock is held.
Four ops cover all fifteen sites — `:assoc`, `:assoc-new` (presence test *inside* the lock, so
a derived method mirror cannot clobber an authored one registered in between), `:dissoc`,
`:cons-new` (the `member?` test inside the lock, for `provide`). Policy stays in Brood; only
the atomicity is kernel. Reads are completely untouched, which matters: dispatch reads
`*impls*` on every call, and the registries cannot become `Table`s because a table deep-clones
values in and out, putting a closure copy on the hot path.

**Two earlier attempts, both reverted** — see the devlog. Optimistic retry cut the loss 44% →
20% but cannot close the read-write window. An in-Brood ticket lock on `table-incr` reached
`LOST=0` and then made `make test` *worse* than the bug: a bounded busy-wait burnt 157 s of CPU
and still lost one under load, and adding `sleep` exposed that a timed-out waiter never bumps
`:served`, desynchronising the sequence permanently.

**One trap worth recording**, because it cost a debugging cycle: `def` binds at
`env_root(env)`, which is **not** always `EnvId::GLOBAL`. During prelude load the root is a
bootstrap env whose bindings later *seed* the shared runtime, so the first version of the
primitive — which wrote straight to the globals table — had its writes silently discarded at
seed time, and the prelude lost its own `Display`/`Inspect` impls (`to-str` then failed with
"no impl for :string"). A kernel primitive that stands in for `def` must read and write the
same place `def` does.

**Verified:** the probe goes 218/500 lost → **0 lost** at 50/200/500/1000, in 0.1 s; prelude
registries are back to their pre-change counts; regression tests for concurrent `impl` and
concurrent `provide`; suite green.

## KI-20 — a JIT fast link ran the callee against the **caller's** IC block · **fixed 2026-07-30**

`jit_run_fast_link` set `jit_call_env`, `jit_dbg_fn`, `jit_native_depth` and the stack limit
before entering the callee's native code — but not `ic_bases`, which the cloning native-link
path in `jit_dispatch_call` *does* set. `FastLink` (the IR-readable fast-path mirror) carried
no callee block, unlike `CallIcEntry`. So callee B ran with arm A's `cur_ic_base`/`cur_gic_base`,
and B's `vm_call_ic_put` / `vm_global_ic_put` / `vm_fast_link_publish_native` wrote into A's
slots, and vice versa.

**Never a wrong answer.** Every read path re-validates `(sym, argc, epoch)`, so a crossed
entry simply misses. The cost was that both arms ran with a permanently cold cache; it also
made `dbg_site_loc` and `[jit-staged-stale]` report the wrong source position.

**The fix (the one the earlier reverted attempt spelled out).** The callee's block bases now
**ride in the `FastLink` slot** — `_pad` became `callee_ic_base`, and a `callee_gic_base` was
added (the struct grows one 8-byte slot). `vm_call_ic_fast_link` stamps them from the entry's
already-resolved `CallIcEntry::callee_bases` (no `vm_arm_block` call on the publish path, no
lookup on the memoised hot path), the IR loads them alongside `code`/`nslots`/`env` (two extra
`u32` loads from the same cache line) and passes them as two more args to `brood_rt_fast_frame`,
and `jit_run_fast_link` does `set_ic_bases(callee_bases)` around the native call and restores —
exactly as the cloning path already did. So the runtime callback **never re-reads the table**:
the whole install is two `Cell` writes off values it was handed. A native flat cell carries
`(0, 0)` (a builtin runs no IC-using arm).

**Why the first attempt regressed and this one doesn't.** The reverted version read the bases
inside `jit_dispatch_fast_frame` via a `RefCell` borrow + a bounds-checked index **per call** —
on the very path whose purpose is to skip the IC probe, and dominated by self-recursion where
the install is a no-op — costing `bintree` +5.5% for no gain. Riding the bases in the slot adds
no lookup: a pinned best-of-9 A/B (`fib`, `bintree`) measured **+0.0%** against a +0.0%
base-vs-base floor.

**Guard.** A debug cross-check in `jit_dispatch_fast_frame` asserts the IR-passed bases equal
what the authoritative IC resolves (`b == callee_bases`), so a future mirror desync trips in
debug across the whole suite. The correctness net is the full differential + jit suites (the
change can only make a callee read its *own* caches, never produce a wrong value).

---

## KI-19 — the VM resolved a call's free-global head **after** its arguments · **FIXED 2026-07-30**

**Fixed.** The tree-walker evaluates the operator first; the VM elided a free-global head
and resolved it through the call-site IC *after* the arguments ran, so an argument that
rebound the head made the engines disagree — `(f (bump))` gave `:new` on the VM and `:old`
on the tree-walker.

`Inst::Call` now carries a `staged` flag. The compiler stages the head (as a `GlobalIc`,
resolved at IC speed) ahead of the args for exactly the calls that can be affected: a
**rebindable** global head with at least one argument that can run user code. `head` and
`site` stay populated, so the call-site IC still caches the resolved **arm** — validated by
closure identity against the staged callee, one compare on the common path. The JIT treats
a staged head as a computed callee (`head: None`, `site: NO_SITE`) so it calls the staged
value rather than re-resolving.

Three things had to be got right, and each was learned by measuring a wrong version:

* **Don't demote to the plain computed-callee path.** What the elided head really buys is
  the IC's cached *arm*, not the callee lookup: dropping it cost `json` 168 → 1159 ms (6×).
* **`head: None` with a live `site` aborts the JIT.** `emit` decides elided-vs-staged from
  the callee node while `jit_lower` decides it from `(head, site)`; a mismatch made the JIT
  resolve a head that wasn't there — `json`/`bintree`/`nqueens`/`wordcount` all died in
  `brood_rt_call_slow` → `unbound_error` → non-unwinding panic.
* **Exempt reserved names.** `def` refuses a name the language ships (ADR-166), so its
  resolution cannot change mid-call and the elided head stays correct. Staging every call
  regressed `regex` 31%, `wordcount` 11% and `sieve` 9% — almost entirely `first`/`rest`/
  `str`-class calls that were never rebindable.

Rows flat on an idle machine: `json` −3.6%, `bintree` −2.0%, `fib` +0.4%, `nbody` +0.1%,
and the three largest movers at baseline on a solo re-run (`regex` 95.7 ms, `wordcount`
50.6, `sieve` 49.7). `Inst::Call`'s stale doc comment claiming eval order was unchanged is
gone with the change that made it false.

Regression test: `tests/vm_call_head_order_test.blsp`.

---

## KI-18 — a deopt could re-run a `table-put` · **FIXED 2026-07-30**

**Fixed.** Four distinct paths let a JIT deopt re-execute an effect; the first three were
closed earlier the same day (no journal for a call-free effectful arm, a multi-arity
self-call exemption that ignored argc, and the leaf inliner splicing an effectful callee
into an engine that cannot journal). Those made the corruption unbounded and proportional
to the workload — 200 000 iterations put 402 047 times.

The fourth was the residual recorded here, and it is now fixed too: **`jit_run_fast_link`
treated an IC re-probe miss as "give up and let the caller redo the call"**. By then the
callee's native code had already run, so the caller's `brood_rt_call_slow` executed it a
second time — effect included. It showed up as a bounded over-count of exactly 16 (the
`DEOPT_BAIL_CONSECUTIVE` threshold, after which the arm bails and the duplication stops),
independent of iteration count. The IC probe is only an *optimisation* for locating the
arm, so a miss must not change behaviour: it now resolves the arm by name
(`env_get` → `compiled_arm_for`) and takes the same checkpoint-resume path as a hit.

Found by elimination, after instrumenting every re-run path: the deopt paths all resumed
correctly at their journal, `[fl-fallthrough]` was the one that fired 16 times, and
counting entries to *each arm separately* showed the caller was re-entered too — which is
what pointed above `f` rather than inside it.

Also hardened while here: the checkpoint resume now covers **preempt** (outcome 2), not
just deopt. A preempt normally lands on a back edge where the journal is 0, so this is a
no-op there; but if one ever landed after a completed call or a `table-put`, the ip-0
re-run would have repeated it.

Regression tests: `tests/jit_effect_once_test.blsp`, four shapes including the multi-arity
one, each asserting the effect count equals the iteration count exactly.

---

## KI-17 — `nest check` validated qualified names against ITS load set, not per-file reachability · **FIXED 2026-07-30**

**Fixed (ADR-189).** `check-file` now takes a per-file **reachability set** — the
file's *transitive* require-closure — and flags a **user-written** qualified
reference `mod/name` whose `mod` is outside it (bound in the image only by load-order
luck). `std/tool/project.blsp` builds the module→direct-requires graph once
(`%module-direct-requires`, one native parse per file), closes it transitively per
file, and threads each file's set through the fresh / cached / structured check paths
(carried as data in the parallel chunks). Soundness — the checker's cardinal rule —
held zero false positives across the whole `std/` + `tests/` sweep: the transitive
closure clears legitimately-transitive references (a test that `(:use editor/treesit)`
naming `face/…`, since treesit requires `editor/face`), the guard restricts the lint
to references the user *literally wrote* (never a macro-injected one), and a genuine
circular case (`coverage` naming `project/…`, which can't `(require 'project)` without
a cycle) is resolved by a *lazy* runtime `(require 'project)` in the one function that
uses it. Regression tests: `ki17_flags_a_qualified_reference_to_an_unrequired_module`
(Rust) and the end-to-end `nest check` on a project with a bad reference. Suppress a
deliberate exception with `(check-allow :unrequired form…)`. Original report:

**Symptom.** A qualified call to a module the running program never loads —
`(path/basename f)` in a file whose module neither `(:use path)` nor
`(require 'path)` — passed `nest check` clean, then raised
`unbound symbol: path/basename` at runtime. Hit twice in one day downstream
(myedit: `path/basename`, then a cross-module form value with the same shape).

**Cause.** `check-project` loads the whole project image (sources + test files)
before checking; any file's `require` binds the qualified name image-wide, so the
checker resolved it for EVERY file — including files whose own load graph never
pulls that module in. The check answered "bound in the check image", not "bound when
this module is reachable".

**Workaround (pre-fix).** Discipline: every qualified `mod/…` call needs a
`(require 'mod)` in that file (a bare require avoids `(:use …)` import shadowing,
e.g. std path's `join` vs the prelude's) — which the lint now enforces.

## KI-15 — `impl` silently misregisters a **bare** record id · **FIXED 2026-07-27**

**Fixed:** `impl` and `:sealed` now share one helper, `ability--id-kw`, which qualifies a
bare symbol against `(current-ns)` (`circle` → `:<ns>/circle`), keeps a keyword id
untouched (`:int`, `:default`), and never double-qualifies an already-`/`-qualified
symbol. So `(impl S circle …)` registers under the same `:<ns>/circle` a value presents.
Regression tests: `tests/ability_test.blsp` "impl id qualification (KI-15)" (a bare id
dispatches) and `sealed_ability_bare_impl_id_qualifies_ki15` (a bare impl satisfies a
sealed member). Original report:


`ability`'s two macros disagree about whether a bare record symbol gets namespace-
qualified, and the mismatch fails *silently* at registration:

```lisp
(defmodule geometry (:use ability))
(defrecord* circle (r))
(defability Shape :sealed [circle rect] (area [self] :-> float))  ; :sealed QUALIFIES → :geometry/circle
(impl Shape circle (area [c] …))                                  ; impl does NOT     → :circle
(area (circle 2))
;; error: ability Shape/area: no impl for :geometry/circle — have (:circle)
```

`defability--sealed-vec`'s member mapping qualifies a bare symbol against
`(current-ns)`; `impl`'s `id-kw` is a plain `(keyword (name key))`. Since
`identity-of` produces the *qualified* `:module/name` keyword, the bare form
registers under a key no value ever presents — so the impl never matches, the
sealed-exhaustiveness check reports the member as unimplemented, and the failure
only surfaces at the first call.

**Workaround:** always write the id qualified — `(impl Shape geometry/circle …)`,
which is what `tests/ability_test.blsp` and the documentation do.

**Fix direction:** qualify a bare symbol in `impl` the same way `:sealed` does (a
symbol with no `/` gets `(current-ns)` prepended), so both macros agree. A keyword
id (`:int`, `:default`) must keep passing through untouched, and an already-
qualified symbol must not be double-qualified. Alternatively make a bare,
unqualifiable symbol an expansion-time *error* rather than a silent
misregistration — but qualifying is the ergonomic answer and matches `:sealed`.
Either way, add a test that a bare id and a qualified id reach the same impl.

**Found by:** writing the ADR-168 documentation and following the `impl` docstring
verbatim on a first attempt (docs run, 2026-07-27).

---

## KI-16 — the LSP still matches the retired `defprotocol`/`defimpl` · **FIXED 2026-07-27**

**Fixed:** `introspect::protocol_ops` now scans both `*abilities*` and `*protocols*`, so
`impl` op-completion works and `:implements` hovers still resolve; `completion.rs`'s
`enclosing_impl` matches `(impl …)` (was `(defimpl …)`); `definition.rs`/`module_ref.rs`
match `"defbehaviour" | "defability"` (was `| "defprotocol"`). Regression:
`offers_ability_ops_inside_impl`. Original report:


ADR-168 removed `defprotocol`/`defimpl`, but the language server was not migrated
with them:

- `definition.rs` (`enclosing`/interface goto) and `module_ref.rs` match
  `"defbehaviour" | "defprotocol"`. The `defprotocol` arm is dead code — harmless,
  but it will never fire again.
- `completion.rs` offers an interface's ops inside a `(defimpl …)` form
  (`enclosing_defimpl`). Since `defimpl` no longer exists, **op completion inside
  an `(impl …)` form is simply missing** — the one user-visible loss.
- `completion.rs`'s test seeds `*protocols*` directly and is named
  `offers_protocol_ops_inside_defimpl`.

Nothing is broken for `defbehaviour` (goto + hover over `(:implements …)` still
work); abilities are never claimed in a module header, so they were never part of
that path. The gap is purely the `impl` completion affordance.

**Fix direction:** rename `enclosing_defimpl` → `enclosing_impl` and match `impl`,
reading ops from `ability/*abilities*` (via `introspect::protocol_ops`' ability
sibling) as well as `*protocols*`; drop the `defprotocol` arms from
`definition.rs`/`module_ref.rs`. Update the test name and seed.

**Found by:** the documentation validation pass for ADR-168 (docs run, 2026-07-27).

---

## KI-14 — the RUNTIME collector re-walked a deep process's whole root stack at every safepoint · **found 2026-07-27, fixed 2026-07-27**

**Symptom.** `make test` could not go green: `brood::suite brood_suite_passes` was
SIGKILLed at its 600 s nextest cap, deterministically. One-line repro, which hung
indefinitely (observed >10 min):

```sh
nest test --only 'test:every n_ document'
```

**What made it confusing.** The same work outside the test framework was fast and correct:
the identical scan over the identical 318 corpus files returned `[0 188 nil]` in seconds,
each of the 188 `n_` documents parsed fine one-per-process, and the two deep-nesting
documents parsed cleanly inside a spawned green process. It needed the framework context —
and, it turned out, nothing about the framework except **how much code it had loaded**.

**Root cause.** The ADR-091 RUNTIME two-generation collector's cooperative drain report
(`Heap::report_gen_liveness` → `runtime_gen_referenced_private`) probes in two phases:
Phase 1 over the process's private roots and live arms, Phase 2 over its whole LOCAL heap.
Phase 2 already had a stale-dirty throttle (`P2_REVALIDATE_STRIDE`) added for exactly this
class of problem. Phase 1 was deliberately unthrottled, on the premise that it is the
*cheap* probe.

That premise has an unbounded term: `roots` is the VM operand/env stack, so **Phase 1's
cost is O(recursion depth)**. Parsing a 100 000-level JSON document makes it enormous, and
instrumenting the hung run showed the scale — inside a **single** drain epoch:

```
drains: 1   phase1 walks: 78409
[rt-dbg] phase1 roots=1727686 env=2 dyn=0 arms=66448
```

78 409 walks over a 1.7-million-entry root stack. Three compounding faults:

1. **The Phase-2 stale-dirty short-circuit sat *between* the phases.** A process dirty via
   Phase 2 therefore paid a full Phase-1 walk on every safepoint and then discarded the
   result — the verdict is `true` regardless of what Phase 1 found. Pure waste, invisible
   while Phase 1 really was cheap. This was the dominant cost.
2. **Phase 1 had no throttle of its own,** so a process dirty *via Phase 1* (deep inside
   draining-generation code) re-walked O(depth) every reporting safepoint.
3. **`live_vm_arms` was walked per active frame, not per distinct arm.** It is a per-frame
   stack, so a recursive function appears once per frame — 66 448 entries that are a
   handful of distinct `Arc`s, each arm's whole IR tree re-walked every time.

Why loaded-code volume was the trigger: the drain only arms once the shared RUNTIME region
crosses `BROOD_RT_GC_FLOOR`. Few files loaded → no drain → no probe → 520 ms. Whole suite
loaded → drain armed → the above. `BROOD_RT_GC_FLOOR=100000000` (collector off) took the
hung repro to 2.2 s, which is what confirmed the subsystem.

**Fix.** Hoist the stale-dirty short-circuit above Phase 1 (changes no verdict — it only
skips work whose result was already discarded); give Phase 1 its own `P1_REVALIDATE_STRIDE`
throttle, gated on `P1_LARGE_SEED` so a shallow process keeps reporting on its very next
safepoint (the promptness the drain-completion tests rely on); and dedup `live_vm_arms` by
`Arc` identity. Soundness is the same argument Phase 2 already made: a stale-dirty verdict
only *delays* drain completion, it can never fabricate a clean ack.

**Result.** The filed repro: hang → **9.8 s**. Whole in-language suite including
conformance: **3591 tests, all passing, 88 s**.

**Guarded by** `tests/jit_deep_recursion_test.blsp` (deep recursion under the collector)
and the existing `crates/lisp/tests/runtime_collector.rs` drain-completion tests, which pin
the promptness the seed-size gate preserves.

**Residual — the canary's timeouts were too tight for the grown suite (2026-07-29).** The
hang is fixed, but the guard tests began hard-killing at the framework's 120 s per-test
ceiling under the *grown* suite (3591 → 3800+ tests). Not a regression — a false timeout: the
guard-page fix now makes deep recursion DEOPT to the VM and raise the catchable
`MAX_BC_FRAMES` error (→ the parse returns `:rejected`), so the worker is *slow*, never dead.
With the drain armed (the whole suite's loaded code arms it — the very condition under test)
one parse costs ~36 s of CPU (measured at `BROOD_RT_GC_FLOOR=1`), and `:serial` serialises
only *within* the group, so the warm passes + spawned parse compete with the rest of the
suite for cores; the whole-test wall-clock overran 120 s (the suite still *completed* — no
guard-page abort, the real KI-14 symptom). Fixed by tagging the group `:slow` (the
`*test-slow-timeout-ms*` = 900 s batch budget the conformance corpora already use for work
that is long, not stuck) and widening the in-test `after` liveness nets to 850 s (a stuck
worker is still caught, cleanly, under the ceiling). The warm passes were kept — they are
load-bearing (the spawned parse must hit the WARM native path, the condition the canary
guards). The *underlying* residual — a deep-recursing process still pays a
throttled-but-O(depth) Phase-1 drain-report cost — is bounded and left for a separate,
ADR-gated collector-perf pass if it ever bites outside this canary.

**Found alongside** two genuine stack-overflow aborts on the same corpus, both fixed and
both distinct from the hang (each aborts rather than hangs, and neither is the reason
`make test` was red):

- **A JIT'd arm could run the native stack into its guard page.** The pre-existing guards
  (`jit_native_depth` + the `stacker` headroom probe) sit on the *dispatch* paths, so they
  only bound recursion that goes through a fast link; recursion via `brood_rt_call_slow`
  re-enters Rust every level while the depth counter stays near zero. Fixed with a
  three-instruction prologue guard in every lowered arm, checking the frame's address
  against a `Heap::jit_stack_limit` stamped from the live remaining stack at each native
  entry. On a trip it sets `jit_force_vm` and deopts, so the subtree drains through the
  VM's bounded heap frames and raises the clean, catchable `MAX_BC_FRAMES` error. (Deopt
  alone livelocks: the VM re-runs the arm, the callee re-tiers, the prologue trips again.)
- **`gc_runtime::flush_rt_value` ⇄ `flush_rt_pair` recursed unguarded.** The cdr spine was
  already iterative, but *car* nesting recursed one native frame per level, so promoting a
  deeply nested value into the RUNTIME region aborted the thread. The LOCAL twin
  (`gc::flush_value`) already had the `stacker::maybe_grow` guard; the RUNTIME one was
  simply missed.

**Adjacent finding, now its own entry and fixed:** `nest observe`/attach tests leaked a `brood`
child — one found alive 2h22m after its run (`/tmp/brood-observe-<pid>/target.blsp`, ~2.7% CPU).
Independent of this bug. Recording it *here*, inside a fixed entry, is what made it read as closed
for nine days; it is **KI-29** now, and fixed 2026-08-05.

## KI-13 — cross-module return-type inference blows up exponentially in branch count · **FIXED 2026-07-27**

**Fixed — it had TWO independent causes, both now addressed.**

*Cause 1 — type size.* Inferring an undeclared recursive value-builder's return unions
branch results into a `Ty` with a deep, `Arc`-shared refinement DAG; `==`/`Hash`/
`is_subtype` (recursive, no sharing dedup) walk it as a tree → superlinear per op.
`Ty::bounded` now widens any `Ty` whose refinement tree exceeds `MAX_TY_NODES` (64) to its
flat tag set — a sound over-approximation — applied at `union`, `intersect`, and every
structural constructor (`seq_of`/`map_of`/`record_of`/`tuple_of`), so no `Ty` ever gets
large. Fixed the moderate case (4-branch `deriv`: 25 s → 0.3 s). Regression:
`inferred_type_size_is_bounded_ki13`.

*Cause 2 — walk count.* With size bounded, the full 5-branch `deriv` was still dominated by
exponential **re-inference**: `sig_of`/`infer_sig` had no result cache, so a callee's body
was re-walked on every request (~400k walks), and because the cycle guard is slipped by a
self-call stored as a bare name vs the qualified name a caller resolved, the re-walks
compounded. `infer_sig` now **memoizes** completed inferences per check pass (`SIG_MEMO`,
cleared per `check_file`), capping it at one walk per distinct name. Full 5-branch `deriv`:
>900 s → 1.1 s.

Both are sound (widening / a deterministic memo for a read-only pass): 255 checker tests
plus the full app ecosystem (hatch 750, brood-chat 102, hatch-demo 89, …) unchanged. The
`(sig deriv (any -> any))` in the corpus is no longer load-bearing (kept as API hygiene).
Original report:


**Symptom.** `nest check` never finishes. No diagnostic, no progress output, one core
pegged. The LSP is the same code path, so an editor hovering the call site hangs with it.

**Trigger.** A recursive function with several `cond`/`if` branches that build *nested
list structure*, **called from another module**. The same call inside the defining file
checks instantly — this is specifically the cross-module path, where the checker infers an
undeclared callee's signature (`sig_of` → `infer_sig` → `expr_ty` over the whole body).

Minimal reproduction — this is the `deriv` benchmark from the Gabriel corpus
(`tests/support/gabriel/deriv.blsp`), which is how it was found:

```lisp
;; a.blsp
(defmodule zzm)
(defn d (a)
  (cond
    (not (pair? a)) (if (= a 'x) 1 0)
    (= (first a) '+) (cons '+ (map d (rest a)))
    (= (first a) '-) (cons '- (map d (rest a)))
    (= (first a) '*) (list '* a (cons '+ (map (fn (b) (list '/ (d b) b)) (rest a))))
    (= (first a) '/) (list '- (list '/ (d (second a)) (third a))
                             (list '/ (second a) (list '* (third a) (third a) (d (third a)))))
    else (error "no")))

;; b.blsp — checking THIS file is what hangs
(defmodule zz-probe (:use zzm))
(def r (d '(+ x 1)))
```

**Measured scaling** (release `nest check`, one recursive branch added at a time):

| recursive branches | `nest check` |
|---|---|
| 2 | instant (105 ms, the floor) |
| 3 | 105 ms |
| 4 | **8.7 s** |
| 5 | did not finish (killed at 900 s) |

So each additional branch multiplies the work by roughly an order of magnitude —
consistent with the inferred `Ty` growing multiplicatively as `expr_ty` unions branch
results whose element types are themselves nested unions, with the lattice operations
(`union`/`is_subtype`) then superlinear in that size. Not diagnosed further than the
scaling; nobody has profiled it yet.

**Not a hang in the sense of a loop.** `InferGuard` (`types/check/sigs.rs`) already breaks
recursion *cycles* correctly, and `expr_ty` has a depth cap. What is missing is a bound on
the *size* of an inferred type.

**Workaround, and why the corpus port uses it.** A **declared** signature is consulted
before body inference (`declared_heap_sig`), so it short-circuits the blowup entirely:
adding `(sig deriv (any -> any))` takes the case above from >900 s to 105 ms. That is what
`tests/support/gabriel/deriv.blsp` does, with a comment saying so — the sig is honest and
declaring a public API's type is right anyway (ADR-153), but it is load-bearing there and
must not be removed as decoration.

**Likely fix.** Cap the size of an inferred type and widen past the cap. Widening an
over-approximation is always sound, so this cannot introduce a false positive; it can only
lose precision on exactly the pathological shapes. The existing `MAX_INFER_DEPTH` is the
precedent for the shape of the fix.

**Severity.** `nest check` is a CI gate and the checker also backs the LSP, so a hang is
worse than a wrong warning — and the trigger is ordinary code, not something exotic. No
regression test yet; the reproduction above should become one with the fix.

---

## KI-12 — a frozen prelude global's inner handle resolved to the wrong object · **found + fixed 2026-07-26**

**Symptom.** The prelude's only non-trivial global value is corrupt in every build:

```lisp
(println (pr-str *load-path*))
;; expected: (".")
;; actual:   ("A list of the given arguments.")   ← `list`'s DOCSTRING
```

The *list* is right (one element, `rest` is nil) — its **car** is a different object
entirely. Which object depends on heap layout, which is what makes it a handle bug
rather than a logic bug:

| build / flag | `*load-path*` |
|---|---|
| debug, default | `("A list of the given arguments.")` (a docstring) |
| debug, `BROOD_GC_VERIFY=1` | `("ret")` |
| prelude written `'(".")` instead of `(list ".")` | `(xs)` — a **symbol** |
| **`BROOD_VM=0`** (tree-walker) | `(".")` — **correct** |

So the car slot holds whatever value happens to live where the handle points, and
the kind bits change with it — i.e. the stored `Value` is wrong, not merely
mis-read. Correct under the tree-walker, wrong under the VM.

**Not a regression.** A `brood` binary from 2026-07-25 18:49 (predating that day's
syntax work — it still rejects `^x` pins) shows the same class of failure, with
`cond`'s docstring in the slot instead. The 2026-07-26 prelude edits (alias trims,
`sig` adoption) only changed the *layout*, hence which wrong object appears.

**Why it hid for so long.** The prelude has essentially one global holding a heap
value, and nothing reads it on the happy path: `require` finds every std module via
`%builtin-module`, and a project run replaces the path wholesale
(`project-setup` → `set-load-path!`). So filesystem module lookup from the
*default* path has been broken without a single test noticing — except
`brood-lsp`'s `completion::tests::completes_module_names_in_require_and_use`,
which puts a temp dir on the default path and asks for completions. That test is
the canary and is **currently failing**.

**Root cause.** `to_prelude` (`crates/lisp/src/core/heap.rs`) re-tagged a handle by
keeping its slab **index** and changing its **region** bits. That is valid only for a
LOCAL handle, because the builder's slabs *become* the prelude region — and it was
applied unconditionally. Instrumenting the freeze showed the offending global was:

```
[freeze] *load-path*: Pair region=0 idx=55990 car=Str region=2 idx=60
```

a LOCAL pair (region 0) whose car was a **RUNTIME** string (region 2). The VM interns
its constant-pool literals into the shared RUNTIME region so compiled code can be
shared between processes, so `(def *load-path* (list "."))` — evaluated by the VM
during the prelude build — bound a pair holding a RUNTIME `"."`. Re-tagging turned
that into PRELUDE `Str@60`: a different slab, an unrelated string. The tree-walker
was correct because it passes the LOCAL string straight from the read form.

**Fix.** Two halves, both in `heap.rs`:

1. `Heap::localize_for_freeze` — before anything is re-tagged, deep-copy any part of
   a global's reachable graph that lives outside LOCAL into the builder's own slabs
   (a forwarding table collapses shared structure and terminates cycles). Reachable
   prelude state is then all-LOCAL by construction, which is what the re-tag assumes.
2. `to_prelude` re-tags **LOCAL only** and returns anything else untouched. The slab
   sweep visits every cell including unreachable boot garbage, which may legitimately
   hold RUNTIME handles; flipping those is what did the damage. A `debug_assert` on
   the root bindings catches a future `localize` gap at freeze time instead of
   silently corrupting a global.

Cost: one walk of the reachable global graph per *source* boot (freeze 5.0 → 8.7 ms,
source-boot peak 3.7 → 10.6 MB, both once per build — the cached boot is unchanged at
~49 ms). All three boot paths now agree: `*load-path*` is `(".")` under the source
boot, the cached boot, and `BROOD_VM=0`.

**What it had been costing.** Filesystem module lookup from the *default* load path
never worked — `require` found std modules via `%builtin-module` and a project run
replaced the path wholesale, so nothing noticed. `brood-lsp`'s
`completion::tests::completes_module_names_in_require_and_use` was the only test that
read the default path; it passes again.

## KI-11 — JIT tail-chain recursion escaped the native-depth cap · **found 2026-07-25, fixed 2026-07-26**

**Symptom (before the fix).** Deep non-tail recursion on the **JIT** path overflowed the
*native* stack and killed the process — `thread '<unknown>' has overflowed its stack /
fatal runtime error: stack overflow, aborting`. An abort, not a panic, so the crash-dump
hook (`install_crash_dump`) never fired and there was no `.brood_crash_dump` to read.

The other two engines were correct on the identical input, which is what identified it as
a JIT bug rather than a missing guard in general:

| engine | 20,000-deep nested JSON |
|---|---|
| default (JIT) | **process aborted** — native stack overflow |
| `BROOD_NO_JIT=1` (bytecode VM) | parsed fine — frames are heap-backed |
| `BROOD_VM=0` (tree-walker) | clean, catchable `recursion too deep: used 12585200 bytes of stack, over the 12582912-byte budget` |

Threshold was between 10,000 and 20,000 nesting levels. `gdb --batch -ex run -ex bt` put
the recursion in `jit_runtime::jit_run_fast_link` ← `brood_rt_fast_frame`.

**Impact (before the fix).** Any Brood service parsing untrusted nested input was
killable with a few kilobytes — `std/json` the obvious one, but this was a property of
the JIT call path, not of the parser. Unrecoverable: `try`/`catch` could not see it, and
a supervisor could not restart from it because the whole OS process died, not the green
process.

**Repro** (the minimal shape — a three-function cycle with a tail-call delegator,
entered from a non-tail position; see the root cause for why a plain `(defn deep (n) (+ 1
(deep (- n 1))))` runs to 200,000 without tripping it):

```lisp
(defn tv (n) (if (= n 0) [0 n] (ta (- n 1))))   ; enters the cycle
(defn ta (n) (tacc n))                           ; the tail-call delegator
(defn tacc (n) (let ([v j] (tv n)) [(+ v 1) j])) ; non-tail: result destructured
(tv 20000)                                       ; aborted pre-fix; fine now
```

**Found by** the JSONTestSuite corpus (`n_structure_100000_opening_arrays.json`,
`n_structure_open_array_object.json`) — see ROADMAP "External conformance corpora".

**Root cause.** Not a missing guard — a guard that stopped applying. The cap
(`JIT_NATIVE_DEPTH_LIMIT`, 1500) and its counter (`Heap::jit_native_depth`) already
existed and *were* checked by both dispatch entry points. But `jit_run_fast_link`
restored the counter to the caller's level as soon as the native callee returned, and
*then* handled the outcome — and three outcome arms re-enter the evaluator while that
frame is still on the native stack: the outcome-4 tail-chain follow-through
(`apply_value`) and the two deopt/preempt re-runs (`vm_resume_deopt` / `vm_apply`). So a
chain of tail-calling delegators oscillated between `depth` and `depth+1` forever while
the native stack grew without bound. The cap never tripped because the depth never
climbed.

That is why no existing depth test caught it: the trigger needs a **cycle** of functions
in which at least one link is a plain tail-call delegator (so its native returns outcome
4) and the cycle is entered from a non-tail position. Plain deep self-recursion — even
with a 20-local frame, even mutual, even building a 20,000-deep nested value — all stay
under the cap and run to 200,000 fine. `std/json`'s
`json--value` → `json--array` → `json--array--acc` → `json--value` is exactly the shape,
with `json--array` as the delegator.

**Fix, part 1 — make the counter count.** `jit_native_reenter` re-raises the depth for the
duration of each re-entrant call, so the cap sees the true native nesting and chains past
1500 levels drain on the VM's heap-backed frames as intended. The hot outcome-0 return path
is untouched, and the cap is only reached by depths that previously crashed.

**Fix, part 2 — make the cap a measurement.** With part 1 in place the release build was
fine but the *debug* build still aborted, because `JIT_NATIVE_DEPTH_LIMIT` is a frame
**count**, and a count is only ever right for one frame size: 1500 levels is a few MB of
the 16 MB worker stack in release and several times that in debug. Same root flaw as the
bug itself, one level up. `jit_native_headroom_ok` now probes
`stacker::remaining_stack()` and refuses a new native link below a 512 KB margin; the
count cap stays as the cheap first test, and the probe is skipped below 64 levels so the
hot shallow path (`fib`, `primes`) pays nothing beyond the existing integer compare.
Measured flat: primes-to-200k 57→58 ms, fib-30 5→6 ms (3-sample, within noise).

This also covers a case neither the count nor this bug report anticipated: a host embedding
Brood on a smaller thread stack. Returning `false` is never a correctness problem — the
caller falls through to the VM, which is where deep recursion belongs.

**Regression test.** `tests/jit_tail_chain_depth_test.blsp` — the minimal three-function
cycle at 30,000 levels, the deeply-nested-JSON rejection, and the plain-self-recursion
shapes that always worked (so a future depth change has to keep them working). Verified by
A/B against a pre-fix binary: pre-fix aborts at 20,000, post-fix runs to 50,000 — in
**both** profiles, debug included, which is what part 2 bought.

The two JSONTestSuite documents that found this are still skipped in the sweep, but on
cost grounds rather than correctness: rejecting a 100,000-level document now means actually
draining it on the VM, which is ~400 ms standalone and >120 s inside the full parallel
suite. That >250× gap is steeper than contention explains and is logged in ROADMAP as a
possible GC-depth pathology worth its own investigation; the property is covered by a
synthetic 5,000-level case.

---

## KI-10 — `receive` compile cliff at the 13th arm · **no longer reproduces 2026-07-25**

Adding a 13th arm to a hot `receive` loop degrades it badly: on `buffer--serve`
(std/editor/buffer.blsp), a single TRIVIAL extra arm (`([:sync-probe] (recur …))`)
took the buffer suite from 4.9 s / 139 MB peak to 8.0 s / 248 MB — +65% wall,
+80% peak — and pushed the full parallel suite over the 1 GB test soft limit.
Bisected 2026-07-11 while adding a `[:sync …]` arm for the resync primitive;
12 arms are fine, 13 fall off the cliff, so the dispatch likely drops from an
indexed strategy to an allocating linear one at that width. Worked around at the time by
merging `buffer--serve`'s two `[:edit …]` arms (buffer-edit always sends the
3-element form) to stay at 12.

**Re-measured 2026-07-25 on `b0b4fd1`: gone.** Adding the same trivial
`([:sync-probe] …)` arm to `buffer--serve` and rebuilding costs nothing, and neither
does adding eight:

| serve arms | buffer suite (`brood --test tests/buffer_test.blsp`) | full suite (`nest test`) |
| --- | --- | --- |
| 12 (committed) | 3.33 s / 55 MB | 22.49 s / 610 MB |
| 13 | 3.35 s / 54 MB | — |
| 20 | 3.36 s / 55 MB | 22.79 s / 636 MB |

Measured on a `cargo build --release` binary with the module **baked in** (the
configuration the cliff was originally seen in — a hot-loaded copy was checked too and
is equally flat). Against the original +65% wall / +80% peak for a single arm, +1.3%
wall / +4% peak for *eight* is flat.

**The mechanism was never identified, so this is an incidental fix**, from some part of
the VM/JIT/pattern work between 2026-07-11 and now — not a change aimed at it. The
arm-count budget is therefore lifted (the note in `buffer--serve` is updated), but if a
width-dependent cliff ever reappears it needs bisecting from scratch: the width alone
was never the trigger. A 13-arm receive of *uniform* trivial arms never showed it
either, which is why the original report is specifically about `buffer--serve`'s arm
shapes.

## KI-9 — arity error from a closure shipped in a `spawn` body · **likely transient; not present in committed code**

Surfaced once while parallelising `nest check`: a driver spawned
`(spawn (send me (list :chunk (self) (chunk-fn chunk))))` where `chunk-fn` was a
*closure passed in as an argument* (captured in the spawned body's env); ~1 worker in
3 died with a bogus `arity error: fn: expected 0 arguments, got 1` at the
`(chunk-fn chunk)` call, silently skipping its files.

**Most likely a transient inconsistent-build artifact, NOT a standing kernel bug.**
The sighting happened while a *concurrent session* was mid-edit on the Rust tree
(uncommitted type-checker changes to `check.rs` et al., committed later as
`be0f8cc`); building from a half-applied edit can yield a binary with a real-but-
transient fault. The decisive evidence is the **frequency collapse**: it died ~66 %
of runs then, and **0 of 50+** now under identical repro conditions (a minimal
captured-closure spawn; the *exact* old shipped-closure `pfold` + `check-file`
chunk-fn at 24 and ~470 workers; the full multi-level `pfold-files→groups→spawn`
capture; all under `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` on a debug-assertions
build — all clean, no tripwire/verifier hit; deterministic arity-through-`spawn`+
`promote` passes). A stable race doesn't go 66 %→0 %; a changed binary does. A code
audit of the promote path (`heap.rs::promote_closure`/`promote_env`) found no defect
— arms/arity copied faithfully, reserve-then-fill with cycle-breaking, lock-free
concurrent append, and RUNTIME compaction can't fire mid-fan-out (uniquely-owned-Arc
gate). **No fix applied** — deliberately not blind-patching the moving-GC/shared-
RUNTIME path to chase a fault not present in the committed code. If it ever recurs on
a *clean committed build*, reopen with that repro.

**Best-practice (independent of this):** prefer shipping **data** to a spawned worker
and resolving the operation through a **global** (as the test runner and
`std/tool/project.blsp`'s `project--pfold-run` do) over capturing a closure value in a
`spawn` body — it avoids the heavier per-spawn `promote` deep-copy of a captured-env
closure regardless.

## KI-8 — RUNTIME form-position table (`positions`) stranded by compaction · **fixed 2026-07-03**

The bug-KI-3 class again, in a side table the KI-3 fix didn't cover. `RuntimeCode::positions`
(source positions of RUNTIME list forms) is keyed by the pair's RUNTIME **slab index**, with a doc
comment asserting "a RUNTIME pair never moves" — a pre-ADR-091 premise the compactor invalidated. A
compaction relocated the pairs but not the keys, so `(form-pos …)` / source-location returned a
stranger's position (or none) afterward. Diagnostics-only. **Fix:** `runtime_collect_with` remaps
the keys through the same `fwd.pairs` forwarding after the evacuation walks, dropping entries whose
pair didn't survive (mirroring the LOCAL `form_pos` remap). **Guarded by:** the `declared_sigs`
regression test exercises the same rewrite pass; the LOCAL analog `form_pos` is remapped in `collect`.

## KI-7 — declared `(sig …)` type-expressions corrupted by RUNTIME compaction · **fixed 2026-07-03**

The bug-KI-3 class in another off-graph holder. `RuntimeCode::declared_sigs` stores each `(sig name
type)` form's type-expression as a **promoted RUNTIME `Value`**, held in a `SymbolMap<Value>` beside
`globals` but NOT walked by `runtime_collect_with`. A compaction relocated the type-expr out from
under the stored handle, so the checker's `sig_of` later read a garbage form (confirmed: `(int ->
int)` read back as `(i 1)` after churn + compact). Silent wrong-data (no tripwire — the handle is a
valid RUNTIME index, just the wrong cell). **Fix:** `runtime_collect_with` now `flush_rt_value`s
`declared_sigs` alongside `globals`. **Guarded by:** `tests/runtime_collector.rs::declared_sigs_survive_a_runtime_compaction`.

## KI-6 — `%isolate` snapshot/restore not RUNTIME-compaction-safe · **fixed 2026-07-03**

A sibling of KI-2 (that fix handled orphan-process reaping; this one the compaction-relocation race).
`%isolate` snapshots the global table — an off-graph `SymbolMap<Value>` of raw RUNTIME handles — runs
a thunk, then restores it. A RUNTIME compaction *during* the thunk (its `def`s crossing
`BROOD_RT_GC_FLOOR`, trivially met in a large image) relocated those handles; the stale snapshot then
reinstalled handles aliasing *other* closures → an unrelated pre-isolate global silently misdispatched
(`foo` → a 1-arg `z-*` defined inside the rolled-back isolate). Latent for every `:isolated` test.
**Fix:** a re-entrant `Heap::rt_collect_block` counter (a `Cell<u32>`) suppresses RUNTIME compaction
while a globals snapshot is outstanding — `snapshot_globals` increments it and `restore_globals`
decrements it, so the invariant holds *structurally* (every caller of the protocol is covered, not
just `%isolate`). Checked at the `runtime_collect_with` choke point, so BOTH the auto safepoint path
(via `rt_gc_due`) and a manual `(runtime-collect)` are covered — an explicit collect inside an
`%isolate` is a no-op, not a corruption. The isolate's `def`s become garbage at restore and are
reclaimed by the next safepoint. **Guarded by:**
`tests/runtime_collector.rs::{isolate_is_safe_against_a_runtime_compaction_inside_the_thunk,
manual_runtime_collect_inside_isolate_is_a_noop}`.

## KI-5 — `nest test` OOMs: shared RUNTIME region accumulates every test file's code · **fixed 2026-07-03**

`run-project-tests` loaded every test file into one long-lived driver image before running any, so
each file's top-level `def`s promoted their compiled closures/chunks into the shared `RuntimeCode`
region + global table — globally rooted, live, unbounded, unreclaimable (only same-name redefinition
frees the old version). A 725-test suite crossed the 1 GB soft cap → `memory limit exceeded` on
whichever workers were allocating (bedit: 9 spurious failures, all passing in isolation).
**Fix:** `test/run-tests-scoped` (+ `-structured`) runs the suite file-by-file, each file inside its
own `%isolate` (reset → load one file → drain → rollback), so the file's `def`s roll back and the next
safepoint reclaims the promoted code — bounding memory to ~one file (relies on the KI-6 fix so the
mid-run rollbacks are compaction-safe). `BROOD_TEST_NO_SCOPE` reverts to the legacy load-all path.
bedit: OOM → 725/725 at 199 MB. **Guarded by:**
`tests/runtime_collector.rs::per_isolate_scoping_bounds_runtime_region_growth`.

## KI-4 — bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote · **fixed 2026-06-15**

A bitset was a blob-backed `Value::Str` holding raw, non-UTF-8 bytes, but
`Value::Str`/`SharedBlob` carry a valid-UTF-8 invariant; promoting a closure that
captured one (`spawn`/`def`) read the bytes through the UTF-8 string accessor →
panic (armed) or UB/`flush_oob`/SIGSEGV (release). Surfaced ~1-in-3 in the
brood-life `--fair` demo. **Fix:** bitsets are a distinct `Value::Bitset` kind with
their own raw-byte slab (LOCAL `Vec` + RUNTIME `boxcar`), byte-clean accessor /
`promote_in` / equality / `Message::Bitset`, mirroring the `bigint` leaf slab — a
bitset can no longer reach a string accessor. **Guarded by:** the spawn-promote-a-
bitset path under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

> **Superseded 2026-06-28:** the whole `bitset` feature (the `Value::Bitset` kind +
> 13 `bitset-*` prims) was removed — it had no in-repo or external consumer left.
> The KI-4 fix is moot now that the type is gone; kept here as the historical record.

## KI-3 — RUNTIME compaction strands live VM / tree-walker constants · **fixed 2026-06-01**

Once the ADR-076 RUNTIME compactor made promoted code-region handles movable, two
sites held them as immovable: the tree-walker elided the operand-stack slot for a
RUNTIME root (so `runtime_collect` never rewrote it), and the VM held promoted
handles inline in `Node::Const`/`MakeClosure.fn_rest` (off the GC root graph). A
compaction at a nested safepoint left them dangling → `flush_oob` or a constant
read back as a different value. **Fix:** `needs_root_slot` (LOCAL **or** RUNTIME)
gives a RUNTIME handle an operand slot; the VM carries movable consts as
`ConstVal::Handle` and registers its live arm in `Heap::live_vm_arms`, which
`runtime_collect` rewrites in place. **Guarded by:** `compile::tests::{const_handle_round_trips,
rewrite_arm_handles_rewrites_every_embedded_handle}` and
`tests/runtime_collector.rs::auto_safepoint_collect_bounds_runtime_region`.

## KI-1 — multi-thread scheduler race: green processes can't resolve globals · **fixed 2026-05-29**

Spawning green processes that touched globals crashed workers with bogus `unbound
symbol` errors (a data race on shared global/scope state via the kernel
supervisor's RESUME_SLOT machinery, worsened by free-list slot reuse). **Fix
(in series):** strip the kernel supervisor (ADR-039, reverted → ADR-044); switch
to a bump-only allocator (slots never recycle, so a stale handle can't observe a
wrong-type value); per-worker pinned queues. **Durable invariant:** no recycled
slots / no stale handles across a safepoint. (The per-worker *pinning* stopgap was
later superseded by ADR-100's heap-captured continuations, which make cross-thread
migration safe and routine.) **Guarded by:**
`tests/concurrency_race.rs::fanout_with_concurrent_global_rebind_matches_serial`
(the fan-out-matches-serial bar) and the self-diagnosing `flush_oob`/`flush_bound!`
OOB check.

## KI-2 — `nest test` flaky / hangs when parallel tests share heavy global lookups · **fixed 2026-05-29**

Two bugs: (1) the KI-1 lookup race could kill a worker; (2) the runner didn't reap
a dead worker, so the run hung in `receive` forever. A 2026-06-07 recurrence under
maximal load was root-caused **not** to a core race but to test isolation:
`%isolate` (test-only) wholesale-restored the globals table, so a test that left an
orphan process running saw the orphan's next lookup die `unbound`. **Fix:** the
runner `monitor`s every worker and accounts for each exactly once (death → a failing
result, not a hang); `%isolate` reaps the processes its thunk spawned (via the
green-friendly `scheduler::yield_now`, never a thread sleep) **before** restoring
globals. Production never wholesale-restores globals, so the language itself was
never implicated. **Guarded by:** `tests/runner_failfast_test.blsp`.

## Platform gaps — GUI display seam · **all resolved 2026-05-31 (ADR-079)**

The GUI frontend had one font size for everything. Resolved: a `Face` carries an
integer `:scale` (per-op/region larger text in a scale×scale cell block — also
covers per-pane font); `gui-font!` takes an optional window id for per-window fonts;
`std/editor/pane.blsp` (ADR-077/078) provides pane layout + clip-rects. Per-pixel
`:height` sizing stays deferred (would break the uniform grid).

## Minor (all fixed)

- **Type-checker noise around `(require 'proc/hatch)`** — `check_file` pre-evaluates
  top-level `(require …)` so the required module's macros resolve.
- **`nest format` collapsed multi-line forms** — fixed (`5b19787`); respects author
  newlines. Still normalizes intra-line multi-space alignment (a standard trade-off).
- **Plain-release segfault on tail-recursive workers** — fixed by per-worker pinned
  queues, then made moot by ADR-100 (heap-captured continuations).
- **`cargo test --test suite` debug segfault** — coroutine stack overflow, not a
  memory bug; `WORKER_STACK_BYTES` raised (pages mmap'd lazily, ~0 cost until needed).

## KI-21 — `nest run --for` / `--watch` emit a pre-ADR-150 `~p` pin

**Status:** ✅ **fixed** 2026-07-30 (`~p` → `^p`). Found while smoke-testing a
TUI/GUI app end to end.

**Symptom.** Any `nest run --for <duration>` or `nest run --watch <path>` fails
immediately, whatever the file:

```
$ printf '(println "hi")\n' > /tmp/t.blsp
$ nest run /tmp/t.blsp --for 1s
error: match: `~p` is not a pattern — a pin is written `^p` (`~` belongs to
quasiquote alone). To match the literal 2-element list (unquote x), quote the
head: ('unquote x).
    at error / match-compile-velems ×3 / match-compile-vector /
       match-build-clause / receive
```

Plain `nest run <file>` is unaffected — only the wrapped path.

**Cause.** `crates/nest/src/main.rs:1203` builds the wrapper source as a string:

```rust
"(let (p (%spawn (fn () {}))) \
      (monitor p) \
      (receive ([:down _ ~p reason] (println \"[exit]\" reason)) {}))",
```

`~p` was the pin syntax before **ADR-150** moved pins to `^p` and made `~`
quasiquote-only. The generated `receive` therefore fails to compile at runtime.
Because the wrapper is only emitted when `timed.is_some()` or `--watch` is set,
nothing in the normal `nest run` path exercises it — and since the code is a Rust
string literal, neither `nest check` nor the Brood test suite can see it.

**Fix.** `~p` → `^p` in that format string. Verified: `nest run --for 800ms` over a
trivial file now exits 0 and prints `[stopped after 800ms]`.

**Guard — still owed.** A test that actually *runs* `nest run --for 200ms` and asserts
a zero exit. The general lesson is the durable part: **Brood source generated from Rust
string literals is invisible to `nest check` and to the in-language suite**, so every
such snippet needs an execution test rather than a reading. Worth grepping for the
others.
