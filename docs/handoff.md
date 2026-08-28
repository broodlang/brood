# Handoff — what to do next, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); decisions in [`decisions.md`](decisions.md); the
option book in [`runtime-frontier.md`](runtime-frontier.md); bugs in
[`known-issues.md`](known-issues.md). Read this to pick the work back up cold.

**As of 2026-08-28 (the green-again session).** The previous session's work — `092ba281`
(three require/process defects) and its merge — had been **committed but never pushed**, and
`origin/main` was red in three CI jobs. All of it is fixed and verified; see the devlog entry
"a red tree, and a gate reading the wrong binary". What a cold reader most needs:

- **`make green`'s `.blsp` half was reading the wrong binary (KI-76).** It gated on
  `target/release/nest` while `make release` builds `target/release-fast` — 9 commits of drift,
  and `std/` is `include_str!`'d, so it reported the `defprocess`→`defserver` rename *backwards*
  as two `unbound symbol` failures. Now it resolves the binary by HEAD's sha and treats a stale
  one as a **failure that skips the gates**, not a note beside a verdict. If you see
  `the .blsp gates DID NOT RUN`, that is the new behaviour working — run `make release`.
- **Local clippy is only as good as its version.** CI pins `dtolnay/rust-toolchain@stable` and
  there is no `rust-toolchain.toml`. Four CI errors were lints new in **clippy 1.98** that a
  full `--all-features -D warnings` run on 1.97 passes cleanly. `rustup update stable` before
  believing a local clippy green. (The `--all-features` warning in CLAUDE.md has this companion:
  the *version* arms lints too, not just the feature set.)
- **An adopted `(sig …)` can be less precise than the curated sig it shadows, silently.**
  `(sig capitalize (string -> any))` shadowed the curated `(string -> string)` and switched off
  the `(+ 1 (string/capitalize "x"))` finding. Declared sigs are authoritative, so this loses
  checking with no warning anywhere. Now gated structurally by
  `no_declared_std_sig_widens_its_curated_signature` (returns only — see its doc comment for
  why parameters are deliberately out of scope). **Read that before the next adoption round.**

**KI-72 is FIXED (2026-08-28), and it was not what three sessions thought.** Not a lost message,
not the require protocol, not the scheduler: a section's entries are defined one at a time into
the **shared** global table in `(global-names)` order, so `string/blank?` (position 7) was callable
while the module-private `string/whitespace?` its body calls (position 51) was not — and 17 of 24
`spawn`ed children died `unbound symbol: string/whitespace?`, so the root waited forever for
replies that could never total 24. Source loading is immune because the file defines the helper
(line 190) before its caller (192). Fixed by emitting each section **privates-first**.

Two things to carry forward from it:

- **Read a hung run's own output before theorising.** The dying children print to stderr from a
  *green process*, and libtest captures per thread — so `cargo test`/nextest swallow it and it
  shows only under `--nocapture`. Three investigations used gdb and in-language watchdogs (which
  perturb the timing) and never read the output. Same lesson as KI-64.
- **The image is not default-ON, and the reason is now measured rather than hedged.** The hang is
  gone and the image arm is at parity with the no-image arm on every repro (the original
  12-parallel one went 12/12 over the cap → 0/12). But `(global-names)` order is **alphabetical**,
  so the same window exists for a public calling a sibling public that sorts later — a static scan
  finds **≈257** such calls across `std/`'s 1318 module publics, three verified by hand
  (`datetime/days-in-month`→`leap-year?`, `datetime/today`→`utc-now`,
  `editor/ansi/ansi-clear`→`ansi-clear-screen`). They have not been *seen* to fail only because
  none is on a funnelled autoload path the way `string/blank?` is. **So privates-first is
  necessary and not sufficient.**

  The prerequisite for a default-ON proposal is an **atomic section install**, and it is more
  tractable than it looks: the kernel already has `Heap::root`/`read_root` +
  `roots_len`/`truncate_roots` (`core/heap/gc.rs`), so pass 1 can build-and-root and pass 2
  define-from-root. The trap to avoid is the naive version — buffering built values in a plain
  Rust `Vec` leaves them unrooted while `from_message` keeps allocating (use-after-GC). Validate
  under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. The win waiting behind it, measured on this box:
  `http` 12.93 → 5.98 ms, `json` 8.29 → 3.97 ms, `regex` 4.90 → 1.87 ms (1.5–2.6× on the module
  load every short-lived invocation pays).

**Still open: KI-74** only (⚠️ watching — one unnamed lib-suite failure, 20 clean runs since;
re-run under nextest, which names the case, if it recurs).


Three separate gates turned out to be passing without testing anything, and the theme of
that day is that *all three failed the same way* — see the box after the next section.

**Read this first if you are about to judge whether the tree is green.** It was not, and the
run list said it was. Every CI run after a failure is `cancelled` by the next push (the
workflow's `concurrency` group cancels in-progress runs per ref), so `gh run list` showed a
wall of cancellations with an `in_progress` at the top and no red anywhere — while the last
three *completed* runs had all failed, for two days. **Filter to completed runs.** A
cancelled run is not evidence of anything. **`make green` now answers this** — completed runs
of the CI workflow specifically (`Release` goes green on commits whose CI failed) plus the
local gates `make check` skips, and it prints "no verdict" rather than "green" when CI has
concluded nothing recent. `make green-all` adds the examples and stress corpus gates.

**What was wrong, and what now guards it:**

- **KI-69** — `differential (tree-walker)` had been red since KI-64's fix. Its two new
  `jit_plan` guards assert on VM-compiled arms and that job runs `BROOD_VM=0`, so nothing
  compiles. Both *refuse to pass vacuously*, which is why they failed instead of lying.
  Fixed with the `set_forced_ceiling(Some(Tier::Native))` pin `compile/tests.rs` has carried
  since ADR-222.
- **KI-68** — the fuzz-differential gate had been comparing **dead programs**.
  `stress/fuzz_programs.py` writes Brood from Python and the rename waves retired every name
  it emitted, so every generated program died on `(def t (table))` *identically in all four
  configs* — and a differential reads identical death as agreement. 20 seeds of
  `ok (exit=1)`, then "all configs agree". Names fixed **and liveness asserted**: an unbound
  name in a generated program is now a hard failure, and a run where not one seed exits
  cleanly fails as "the corpus is dead, not the engines agreeing".
- **KI-70** — `nest check` **never walked a vector or map literal**. `check_into_inner`
  opened with `let Value::Pair(_) = form else { return }`, so everything inside `[…]`/`{…}`
  was unchecked by every lint — the entire Hiccup style (`std/editor/*`, every Brood web
  layer). Found because hive's `/docs` renderer had called bare `max` for weeks with a green
  `bin/ci`. Vectors and maps now descend; the first run over `std/` + `tests/` returned
  exactly one warning and it was real (the fifth dead `project-*` call site — the MCP
  `callers` tool, now `project/all-files`).
- **22 harnesses of rename rot** in `examples/` and the stress corpus, and both live docs
  (`language.md`, `brood-for-claude.md`) still teaching `print`/`println`/`eprint`/`eprintln`,
  `spawn-server`, and bare `quot`/`mod`/`rem`/`floor`/`min`/`max`.

> **The one lesson, stated once.** Every failure above is a gate that **could not fail**.
> KI-68's differential compared two identical corpses; KI-70's walk returned before reaching
> the code; the cancelled-run wall hid the reds. A gate whose pass condition can be satisfied
> by doing nothing is worse than no gate, because it is *believed*. Whenever you add or touch
> one, write the assertion that the gate did real work — a minimum count, a non-empty result,
> a liveness check — and **sabotage-verify it**: break the thing on purpose and confirm the
> gate goes red. Both KI-69 guards survived only because someone had already done this
> (`only {checked} lowerable chunks inspected … a green result would mean nothing`).

**All three ungated classes that session listed as open are now CLOSED** — by `5aa49463`,
which landed after the paragraphs above were written:

- The corpora are checked **statically**: `make check-corpora` / `scripts/check-corpora.sh`
  runs `nest check` over `examples/`, `stress/`, `scripts/fuzz/stress/` and `breakage/` and
  fails on any `unbound symbol`, per tree. The three runtime gates beside it
  (`check-examples`, `check-stress`, `breakagetests`) only ever saw a name on an **executed**
  path; with all three green, the static pass found **74** unresolvable names. Wired into CI
  (`ci.yml`), ahead of the slow run-based gates.
- Brood **embedded in non-`.blsp` files** is gated where it mattered — the docs' 123 code
  blocks, via `tests/doc_snippets_test.blsp` (qualified call heads inside code blocks: 24,
  all real). The other two surfaces were already covered (`scaffold_quality.rs` checks every
  `nest new` template; the Python fuzz generator got its liveness assertion with KI-68).
- The **reversed-args class** has its gate (KI-71).

**The toolchain gaps a downstream migration exposed are CLOSED too (2026-08-27, ADR-257).**
Migrating hive and its dependency closure across the namespacing waves took the registry down
twice; neither outage was a language bug, both were gaps in what the toolchain could tell you
before you shipped. Three of the five were a question nothing had a command for:

1. **Does it boot?** `nest run --check-boot` loads every source module and resolves `:main`,
   running nothing; `nest release --smoke` then does it to the **binary just written**. That
   second half is the one that matters — a bundle carries a *snapshot* of every dependency,
   so a dep updated on disk since the last `nest fetch` is invisible to any source-tree check
   and fatal in the artifact. All four entry paths share one entry resolver, so the check
   cannot drift from the boot it checks. (KI-66) **Know its edge:** it catches a module that
   raises at *load*, not a name reached only once `main` executes — that one is
   `nest run --for`, already wired in hive's CI. ADR-257 has the three-way table; don't
   trust `--check-boot` past it.
2. **What is this binary?** `myapp --brood-build-info` — version, build-id, features, app +
   module count. The **`--brood-` argv prefix is reserved** (two names, first position only)
   so the bundle's "argv belongs to the app" contract is intact; it loads no module, so it
   answers on a broken bundle.
3. **What moved?** `nest check --fix-renames` (+ `--dry-run`). Applies only unambiguous public
   moves, through the CST and `:refs-only`; declines with a printed reason for ambiguity, a
   `%`-withdrawn target, and a name the project itself defines — the last being the hazard
   that cost a revert, since `nest rename` is not scope-aware.
4. `nest check` now sees inside a `try` body (KI-67), and `docsite/render-css` emits CSS
   variables so an embedding host rethemes by redefinition rather than by overriding ~30
   selectors.

**So: no open bug, no watch item, and no open item on the last two sessions' lists.** The
tree is clear to start new work — verify with `make green` first (it is the answer to "is
this tree green?"; do not hand-read the run list).

**Where to look next**, in rough value order — see `ROADMAP.md` and `docs/roadmap-for-v1.md`:

- **The 1.0 language surface is freeze-ready** (all four pre-freeze items shipped, ADR-170
  ratified). The one remaining non-language release blocker is **`nest format --check`'s
  comment *hoisting*** — a style verdict nobody has made, not a defect hunt. ~40% of the red
  is the formatter moving a same-line trailing comment onto its own line, which is intended,
  documented behaviour that this tree's authors do not write for. Decide hoisting first;
  `roadmap-for-v1.md` has the measurement, and says not to run the formatter tree-wide before
  that call. (Re-measure: a format sweep landed 2026-08-19 and the figure predates it.)
- **The stdlib surface audit's residue** (ADR-250–253): example coverage ~16% of ~1,150
  functions (each example written is a test gained — `tests/doc_examples_test.blsp` executes
  them), ability seams that cannot reach `rope`/`table`, and the naming seams.
- **VM/JIT**: the cheap end of the compute frontier is mined out; what is left (X-register
  call convention, computed-goto dispatch, the `bintree`/`nqueens` allocation frontier) is a
  multi-session redesign each.

**Previous session's entry follows.**

**As of 2026-08-19 (the green-the-tree session, concluded).** `main` is **green on all five CI
jobs** at `c8dbf0ea` (run 32247618122) — the first fully green run since the ADR-230/231 namespacing
merge. `known-issues.md` shows **no open bug and no watch item**: KI-36 and KI-47 were both found and
fixed this day.

**What landed.** Three CI jobs were red and all three are fixed:

- **KI-36** (the last watch item, unreproduced since 2026-08-07 across 25 idle and 14 loaded runs)
  reproduced in run 3 of a repeated-run gate and is fixed at the root. It was never the nodedown
  stall its entry inferred — B2 opened its dist listener *before* registering `:echo`, so A's
  ping-on-nodeup landed on a name that did not exist yet and was **silently dropped**. `proc/register`
  now precedes `node-start`. Verified by sabotage in both directions.
- **ADR-232** closes the diagnosability gap under it: a message dropped for a registered name no
  process holds now **warns once per name**, at the receiving node — the only party that knows.
  Semantics unchanged (still dropped, Erlang parity intact); default-on deliberately, because a flag
  you must arm before the bug is absent when it matters (that is KI-39, whose *retroactive*
  self-reporting also failed to fire). `BROOD_NO_DROP_WARN=1` opts out.
- **KI-47** — the tree-walker job was a **memory threshold**, not the three `adversarial_test.blsp`
  cases it named. Process-wide allocation reached 1.145 GB against a 1 GiB backstop; raised to 2 GiB
  soft / 3 GiB hard.
- Plus the rot: **108 stale names** across `breakage/`/`stress/`/`scripts/fuzz/stress/` (three
  separate rename waves) and **34 files** across the two format gates.

✅ **The previous session's flagged "start here" is ANSWERED (2026-08-20): the memory growth is
legitimate, not a regression.** KI-47 blamed module count; that is measured wrong. At constant source,
120× the module count costs **1.65×** peak (~60 KB/module), so all 89 stdlib modules account for
~5.5 MB against the +905 MB in question — and the 2026-08-06 entry it leaned on found the *quadratic*
was in **time** (`*features*`, fixed by ADR-216), with memory explicitly **not** per-module. The
namespacing merge is not the cause either: `098a3316` (pre-ADR-230) vs HEAD on the identical harness
is **+4.4% on the VM arm and −11% on the tree-walker** — HEAD is *cheaper* on the arm that went red.
The "4.8×" compared a 2026-05-30 figure against a 2026-08-19 one across a different engine
(tree-walker = **1.38×** the VM, measured) and a different build (debug vs release); those confounds
cover it. And it has receded: the same debug/`BROOD_VM=0` harness now peaks **726.7 / 757.9 MB**
(two samples) against KI-47's **996.6 MB**, back under the *original* 1 GiB cap. **Keep 2 GiB soft /
3 GiB hard** — ~2.6× margin now; 1 GiB would leave ~1.2×. Full working in KI-47.

**The position tables were then measured on REAL code and are NOT the target that breakdown implied
— written up below so nobody re-picks them.** The 2026-08-06 figures (169 MB of 933 MB = 18% of
memory, 24% of load time) come from a synthetic corpus of 1000-line generated modules at 1.15M
entries, whose form density is nothing like real source. On the 38-module stdlib, measured with the
new `(pos-stats)` surface: the two tables are **1.29 MB of an 18.23 MB load — 7.1%** — and an
ablation build that records no positions at all loads in **137 ms vs 146.5 ms, i.e. a 6.5% ceiling
against a 0.7% base-vs-base noise floor**. Since positions cannot actually be removed (diagnostics,
`source-location`, the LSP), any real optimisation buys a fraction of 6.5%. **Do not start here.**

One genuine but small defect was found and deliberately left: the LOCAL `form_pos` keeps its
**high-water capacity** for the process's life — `gc.rs`'s minor-collection path `retain`s in place,
and `HashMap` never shrinks, so 8 000 live entries sat in 25 156 slots. Shrinking is *not* obviously
right: that `retain` is itself a deliberate time fix (the comment there records that rebuilding the
map was O(all positions recorded so far) per minor), and safe hysteresis recovers only ~219 KB, ~1%
of load memory. Recorded, not fixed.

**So the memory thread is closed at both ends** — no regression (KI-47) and no worthwhile reduction
in the position tables. The remaining named options are unchanged: **M2 shared IC tables** (the
option book's #5, 664 B/proc + a warm start, highest value and highest risk) and the **startup heap
image** (the 18.5 s vs Elixir's 2.26 s target; a real project, and the devlog warns the
cheap-sounding AOT version makes memory *worse*). The AST itself — 220 MB of that same 933 MB, the
largest single item — is the thing the image project would actually address.

**Also open, smaller:**

- **Two thin deadline margins**, measured across four suite runs and stable (fixed cost, not
  variance): `inlined_two_stage_swap_then_deopt_stays_correct` at **65.9–71.5 s against the 120 s
  cap (1.68×)** — the worst in the suite, and thinner than anything KI-46's audit found — and
  `completion_never_fails_however_it_is_called` at 75–80 s against its 180 s override, where
  `.config/nextest.toml` claims 10.5 s post-KI-39. Both drive `fib 30` / 96 subprocess spawns; the
  KI-39-shaped fix is to cut the per-unit cost, not the budget. **Any reduction must be proven to
  still tier** (`BROOD_JIT_DUMP_IR=1`), or a slow-but-real test becomes a fast-but-hollow one.
- ~~**The rename-sweep gap.**~~ **Closed 2026-08-20.** `stress/` and `scripts/fuzz/stress/` now gate
  on every PR via `make check-stress` (`scripts/check-stress.sh`, modelled on `check-examples.sh`);
  `breakage/` already had its own CI job. Before the gate existed it found **8** dead files — two
  rename waves deep, and grep had found only 3 of them, because `stress/` had also rotted under
  ADR-230's `string/*` wave. 25 s for all 28 harnesses; verified by sabotage in both branches.
  `benches/` turned out to be gated too, but only by `cargo clippy --all-targets` — which is how the
  `parse_prelude` bench was caught still naming the pre-split `std/prelude.blsp`, one red build after
  the split. **If you run a rename sweep, `make check-stress` and `make check-examples` are now part
  of proving it landed atomically.**

⚠ **Method warning, and the day's real lesson.** Six separate checks produced confident, plausible,
wrong output, and **none of them errored**: `rustfmt --check` piped to `/dev/null` (it writes its
diff to *stderr*, so dirty reads as clean); a `(name ` regex that also matches `defn` parameter lists
and `let` bindings; a stale `nest` after `cargo clean` reporting `cannot find module 'string'`; a
load-generator whose every failure was priority inversion I had created; a hand-rolled normalizer
that desynced on a string literal; and the pre-push hook silently falling back to a 0.3.11 binary.
Concretely: **`nest test` (release) and `brood_suite_passes` (debug, Rust suite binary) are different
harnesses with different allocation profiles** — the former passed 4652/4652 while the latter failed.
Reproduce CI with CI's harness. And do not mutate the working tree while a verification runs; that
invalidated two 25-minute runs here.

**Previous session's entry follows.**

**As of 2026-08-18 (the per-process-memory / cold-call session, concluded).** Everything is
**committed and pushed** (`be50b5f8`); `main` and `origin/main` agree, working tree clean.
`known-issues.md` shows **no open bug** — KI-36 is the sole watch item (seen once 2026-08-07,
never reproduced). Four issues closed on 08-17/08-18: **KI-44** (the `sqrt` call-site inline,
~1.8× on `nbody`), **KI-45** (the stale `examples/editor`), **KI-39** (never a flake — a
fixed cost under a fixed deadline), **KI-46** (the MCP `check` tool took its project from cwd
and so type-checked this whole repo: 87 s → 2.5 s).

**What landed last, and the thread it leaves open.** The session's question was "the most
profitable large perf change", and it went at the cold-call tax:

- **The premise `runtime-frontier.md` named was refuted.** The forwarder ladder reproduces
  (19.45 / 21.05 / 21.90 / 24.45 CPU µs per unit), but it is **not** a first-call effect —
  calling the same arm again in the same process costs the same. There is no warm-up to
  remove, and compilation is not the cost (`BROOD_TRACE_COMPILE` counts a constant ~142
  compiles whether the run spawns 100 processes or 400, so ADR-215's sharing holds).
- **Shipped instead: the `vm_fast_links` mirror is allocated by its only writer**
  (`Heap::fastlink_slot_grown`) rather than pre-grown in `vm_arm_block`. 19,968 of 20,001
  `spawn-live` units now allocate 0 slots instead of 14; **192.6 B/process** measured with
  `(mem-bytes)`; RSS 6364 → 6093 B/process. Time-neutral at *both* ceilings. 992/992 on both
  engines.
- **Open: the rest of M2b** — `vm_call_ics` (64 B/site) and `vm_global_ics` are still grown
  eagerly per activated arm, and sharing them faces the unchanged read-protocol difficulty
  recorded in `runtime-frontier.md` (both tables are hot, on *different* engines — the mirror
  for JIT'd calls, the fat table for interpreted ones, so a lock there regresses every
  un-JIT'd call site). The two named follow-ons are shrinking `CallIcEntry` 64 → ~48 B and
  sharing entries for frozen callees; both are marginal for their complexity at the corrected
  sizing, so **cost them before writing code**.

⚠ **Two measurement traps this session paid for in real time — they generalise:**

1. **`(mem-bytes)` / `(mem-peak)` are the instrument for an allocation question, not RSS.**
   The session's first write-up compared a *measured* RSS delta against an *inferred*
   allocation delta and concluded there was a "1:0.35 allocation-to-RSS discount", then told
   the next reader to size remaining IC work at a third of face value. That was wrong and is
   **retracted** (commit `be50b5f8`): measured directly, RSS tracked the allocation fully.
   There is no discount. Do not resurrect it from an older copy of the frontier doc.
2. **A slot count is not a byte count until you say *when*.** The same structure read 14 sites
   × 48 B at *teardown* and ~4 sites while *parked* — 3.5× apart, and only the parked figure is
   what peak memory sees, because `spawn-live` (spawn all, then release) has all N processes
   parked at once. A workload that ran each process to completion serially would show the
   other. Confirmed by construction: adding 24 call sites to the unit body moved the saving to
   1345.6 B ≈ 193 + 24 × 48.

**Docs reconciled 2026-08-18 (after the commits above).** `runtime-frontier.md`'s per-process
allocation profile and M2b entry still described both IC tables as eagerly allocated and sized
M2b at "~500 B/process"; both now carry the shipped state and the corrected sizing. This
reconciliation is **docs only — no code changed**, so it inherits the verification above rather
than adding to it.

⚠ **Local-run discipline (carried forward from 2026-08-14, still in force).** Bare `make test` /
`make test-both` OOM-kills this box (28-way nextest fan-out); even a capped single-binary run
tipped it over when free RAM was already low. Check `free -h` before any build/test here and
keep it single-process/targeted. See the `ram-pressure-background-suite` memory.

**Previous session's entry follows.**

**As of 2026-08-14 (the auto-derived-imports session, concluded).** Everything is **committed and
pushed** (`a57cc573`); `main` and `origin/main` agree; full in-language suite green (single-process
run, 470s, exit 0), the std-wide `nest check std/**/*.blsp tests/**/*.blsp` gate at zero warnings,
workspace build + Rust tests green.

Landed the **auto-derived-imports follow-up** to ADR-227 — but as *qualified-reference
auto-require*, **not** the bare-name "Design B" `auto-derived-imports.md` originally planned (a
deliberate fork this session; you confirmed qualified-inference is the end state). A **qualified**
reference `mod/name` infers `(require 'mod)` for *any* module, so no explicit `require` line is
needed. **There is no bare-name magic** — a bare `sqrt` with neither a `math/` prefix nor `(:use
math)` stays unbound; a file still `(:use math)` or writes `math/…`, and only the *require* is
inferred away. New `crates/lisp/src/eval/derive.rs` + three hooks in `macros.rs` (eager qualified
macro head, deferred qualified value ref, root-region scan for scripts/REPL); GC re-roots added in
`macros.rs`/`check.rs` because `compile` can now collect. The checker's KI-17 unrequired-module
lint is now obsolete — a qualified ref requires its own module — neutralized to a no-op in
`walk.rs`. Plus **stage 4 (`json`)**: `std/json.blsp` drops its `json-` export prefix (now
`json/decode`, `json/encode`), consumers + the JSON fuzz target updated. The ADR-227 namespacing
program is now **complete** (stages 1–4 + the follow-up); `docs/{decisions,language,devlog,
auto-derived-imports}.md` and `ROADMAP.md` are reconciled to the shipped design.

**No open thread from this session.** The tree is green with no open bug — `known-issues.md` shows
only the KI-36 / KI-39 *watch* items (unreproduced locally). Pick the next milestone item from
`ROADMAP.md`.

⚠ **Local-run discipline (hard constraint this session).** Bare `make test` / `make test-both`
OOM-kills this box (28-way nextest fan-out); even a capped single-binary run tipped it over when
free RAM was already low. Do **not** run any build/test here without checking `free -h` first and
keeping it single-process/targeted — the full-suite verification above ran as one bounded
`nextest -E 'binary(suite)'` process with a RAM alarm armed. See the `ram-pressure-background-suite`
memory.

**As of 2026-08-13 (the VM-contention session, concluded)**, brood 0.3.9. That session's own
summary is below under "the contention session"; the backend-seam entry it replaced follows it.

**As of 2026-08-13 (the backend-seam session, concluded)**, brood 0.3.9. Everything is
**committed and pushed**; `main` and `origin/main` agree. `nest format --check` is **clean across
all 356 files** — the pre-existing backlog (12 files when this entry was first written, 23 by the
time it was cleared) is gone, including nine embedded `std/` modules, verified with both engines.

### The contention session (2026-08-13, latest)

**What landed:** **ADR-224** — a shared compiled arm is now reached through a process-local
`ArmHandle`, fixing **KI-40**: the VM's call path was cloning one shared `Arc<CompiledArm>`
three times per call, so concurrent green processes running the same arm serialised on a single
refcount cache line. `pfib` at ceiling 1 goes **54.4 s → 17.1 s (3.19×)** and now matches
`BROOD_NO_SHARED_ARMS`; `spawn-live` pays a measured, accepted **+1.8%**. Plus **KI-42** (the
breakage suite had rotted to 9 of 23 red — 7 fixed, 2 skipped by name) and a tooling pass:
`make doctor`, `make ab-vm`, `--version` with the build sha, `ab-bench --tier`, `spawn-live`
un-pinned in `parallel_rows`, and the witness's stale-binary trap closed.

**Three things a next session should know:**

1. **`make ab` cannot see a VM-path regression.** It measures the default ceiling, where a hot
   arm is native and the interpreter's call path never runs — it reported KI-40's 3.19× as
   **+1.3%**. Use `make ab-vm` (ceiling 1) for anything touching
   `exec_chunk`/`dispatch`/`vm_run_bc`, and `make doctor` first.
2. **The two skipped breakage files are judgement calls, not neglect** (KI-42):
   `chaos2_tcp_stress` creates its listener in the parent and accepts in a child, so fixing it
   changes what it measures; `chaos_map_volcano` trips the 1 GiB soft limit by design. Clear
   `BREAKAGE_SKIP` to see them.
3. **This class of bug is invisible to every correctness gate**, because the VM's answer stays
   right — only a benchmark moves. That is why the fix carries a refcount assertion
   (`arm_handle_clone_does_not_touch_the_shared_arm_refcount`, sabotage-verified) rather than
   relying on the suite.

### The backend-seam session (2026-08-13, previous)

**What landed:** ADR-221 (the `JitBackend` contract + the decisions hoisted into `jit_plan`),
ADR-222 (execution is a tier ladder with a ceiling — `BROOD_TIER` subsuming `BROOD_VM` and
`BROOD_NO_JIT`), the perf-triage tooling (`make perf-brood`, `std/tool/perf.blsp`,
`brood --debug-flags`, `(vm-stats-reset)`), `ab-bench --json`/`--floor`, and four CI fixes that
took `main` from red to green. Full narrative in `docs/devlog.md`; the plan and its
plan-vs-reality corrections in `docs/backend-seams.md`.

**Green at the end:** `make test` **978/978** and `make test-both` **978 + 978 = 1956/1956** on
the final merged tree; `cargo test --features jit --test jit` **40/40**, and 40/40 again under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`, per increment; `cargo check --workspace
--no-default-features`; clippy `-D warnings` all-targets/all-features; rustfmt; `nest check` 0
warnings; `nest format --check` clean; `tests/perf_test.blsp` 16/16 on both a counter-armed and an
ordinary build; the lowering witness byte-identical across the restructurings. **CI on `main` is
green** (last four runs).

✅ **`nest check` exited 1 at HEAD for three pre-existing advisory warnings — fixed** (the
`docs/md-links` non-tail recursion became a tail-recursive accumulator with 10 characterisation
tests written against the original first, and `repl.blsp`'s guard now uses `symbol?`, because the
checker narrows on *type predicates* and not on truthiness). The historical note follows.

⚠️ (historical) **`nest check` exits 1 at HEAD — pre-existing, and the CI "zero warnings" gate
should be red on main.** Three advisory warnings: `std/tool/docs.blsp` non-tail recursion (×2) and
`std/tool/repl.blsp` `%in-ns` nil. CLAUDE.md documents that batch `nest check` exits nonzero on
*any* warning. Verified against a clean `HEAD` worktree — identical 3 warnings, identical exit 1
— so this session neither caused nor fixed it. Left alone deliberately (restructuring those two
functions vs adding a justified opt-out is a judgement call), but it should not stay unnoticed:
either fix them or the gate is decoration.

**Both previously-owed gates were run on 2026-08-13 and are green:** the GC_STRESS sweep over
the seven concurrency binaries is **37/37** (matching `afe4bcff`) and the fuzz differential
passed. Also run this session, and not usually: `make breakagetests`, `make tsan`, `make loom`.

**Previous session (2026-08-10, spawn-live + publish):** `make test` 974/974 four times, both
engines 1948/1948, in-language suite 4517/4517 on seven consecutive runs.

**What this session changed — and what is left to do with it.** Items **1–2** of
[`backend-seams.md`](backend-seams.md), recorded as **ADR-221**: a `JitBackend` contract
(`jit/{mod,backend,rt,cranelift}.rs`) and the backend-independent lowering decisions hoisted into
`eval/compile/jit_plan.rs`. No generated code changed. **Uncommitted** — the natural split is two
commits, item 1 then item 2, each with its own gate run already done.

Three things a next session should know before touching this:

1. **`scripts/jit-lower-witness.sh` is the new gate for any JIT restructuring.** It prints the
   sorted *set* of arm fingerprints from 13 rows under `BROOD_JIT_DUMP_IR=1`. Diff it before and
   after. The *count* is unusable (installation is async: ±2 on a 78-lowering sweep); the set is
   deterministic. It is the only gate that observes an arm quietly ceasing to lower — every other
   gate checks the answer, and the VM's answer is correct too.
2. **The scalar-register path now reports** (`scalar-register: i64|f64`). It previously emitted no
   `[jit-ir]` line at all, so `fib`/`pfib` read as never-lowered in the check CLAUDE.md points at,
   where absence is the documented signal.
3. **Ordering trap, live in the code with comments on both sides:** `jit_lower_arm` must try the
   scalar path **before** `jit_plan::codegen::plan_general_lowering`. The gate's predicate
   describes `fib`/`pfib` exactly, so consulting it first silently drops them to the VM — correct,
   therefore invisible to `tests/jit.rs` and `make test`. This nearly landed this session.

Items **3–5** also landed. What a next session most needs from them:

- **`make perf-brood` then `(require 'perf) (perf/summary)`** is now the answer to "where does
  the time go". Use **`(perf/measure thunk)`** for anything narrower than a whole process — the
  counters are cumulative from process start and boot is expansion-heavy, so the same program
  read an **84% defer rate cold-cache and 0.8% warm**.
- **Two things `perf/report` will not tell you, on purpose.** It never concludes `:alloc-bound`
  (once an arm goes native its iterations stop being counted while its allocations do not — a
  200k-iteration loop measured `:alloc` 200017 against `:vm-apply` 197), and it refuses to judge
  a rate resting on under 1000 samples. Both were wrong labels first, then guards.
- **`brood --debug-flags`** lists the perf-triage flags from the binary.
- **`make ab --floor`** reports each row's own base-vs-base noise floor and a verdict against it.
  A *stored* baseline was deliberately not built — absolute ms don't compare across runs or
  machines, so a committed number would be a false reference. Still an open question.
- `Engine::ALL` / `Engine::short()` are the seam for a third engine: add a variant and it gets
  bench rows plus the differential and Gabriel corpus with no harness edits. Note the honest
  limit, recorded on the enum: `Engine` is **not** a trait, so a third engine means answering the
  ~7 questions the compiler flags, not implementing an interface.
- **The `JitBackend` surface is the trait and nothing else, as of the review.** It had four
  bypasses — `jit_runtime.rs` calling into the Cranelift backend's unboxed-scalar submodule — now
  three **tiering advisories** (`may_adopt_shared_code`, `declines_inline_upgrade`,
  `note_depth_bail`). They are *associated functions* on purpose: tiering consults them per
  activation and `&self` would take the `GLOBAL_JIT` lock there. That makes the trait
  non-object-safe, which is deliberate and documented. If you add a backend, those three plus the
  six obligations are the whole contract — and obligation 3 includes outcome **5** (the depth
  bail), which the first version of the contract omitted.

**Previous session's work (2026-08-10).** Two `fold` optimisations — `%vector-reduce` (a native
counted loop for vector folds, which also resolves a passthrough reducer like `+` once instead of
per element) and testing a vector first in `fold`'s dispatch — worth **−16% CPU on the published
`spawn-live` row**, taking the BEAM gap 2.8× → **2.5×**. `stall_report` fixed to read per-thread
state, closing KI-38's last loose end. The suite was republished (2026-08-10 run), the
positioning chart rebuilt to aggregate all 27 comparable rows instead of 11, and a per-row trend
chart added. The base-RSS "regression" carried for three runs was chased and **retired** — it is
noise on a metric that swings ~19 MB with boot-cache state.

**KI-38 is diagnosed, reproduced and FIXED (2026-08-08); KI-36 remains the one watch item.**
KI-38 was the boot-wait cluster: three tests that wait for a freshly spawned *debug* `brood` to
become ready, failing *together* under peak suite load. **KI-28 is not a separate watch item** —
it recurred twice and is one of the three, so it is folded in.

**The mechanism.** `build_id_string()` embeds `binary_stamp()`, the running executable's own mtime,
so the expanded-prelude boot cache is invalidated by **every rebuild**, for `brood`, `nest` and all
~50 test binaries at once. A **cold** boot costs **1227–1361 ms against 107–114 ms warm** — ~11x,
and `BROOD_BOOT_TRACE` shows it is entirely macro-expansion (`expand=1.10s` of a 1.227 s source
boot). Every boot sample the entry previously rested on (151 ms idle, 4066 ms worst) was taken on
an already-built tree and was therefore **warm**; the cold path had never been sampled, which is
why the deadline was being compared against the wrong distribution and the failure read as a
stall. Cold cost times the concurrent herd is linear and crosses the 20 s deadline at ~70
concurrent boots, the 30 s one at ~105 — the observed 34.7–35.5 s failures sit at ~120.

**Reproduced deterministically** (the first time this flake fired on demand), then used as the
regression test for the fix:

```
rm -f ~/.cache/brood/prelude-expanded-*.blsp
cargo nextest run --no-fail-fast --features brood/treesit-grammars -j 64
```

**The fix**: `scripts/warm-boot-cache.sh`, wired as a **nextest setup script** (so a bare
`cargo nextest run` gets it too, not just `make test`). One boot of each spawned binary, ~2.4 s,
before ~50 binaries fan out. Same command, same `-j 64`, same cleared cache:

| test (deadline) | before | after |
|---|---|---|
| `clean_peer_exit` (20 s) | **FAIL 20.119 s** | **2.599 s** |
| `drop_guard` (30 s) | 28.363 s | **1.926 s** |
| `pdeath` (30 s) | 26.697 s | **1.991 s** |

Note `-j 64` on 12 cores independently breaks `gc spawned_process_reclaims_too` and times out a
jit case — over-subscription damage, not a regression, and both pass at the default `-j`. Even so
the run improved from *1 failed, 3 timed out, 1 flaky* to *1 failed, 1 timed out, no flaky*.

Two things the fix deliberately does **not** do, both recorded in `docs/known-issues.md`: it does
not warm the ~50 test binaries' own in-process boots (each has its own mtime-keyed file, so they
cannot be warmed without running them — the suite does that anyway); and it does not re-key the
cache on prelude *content* to let all binaries share one file, because the mtime is what catches
an uncommitted change to the expander, which is exactly the development loop this repo lives in.

**`stall_report` was blind and is now FIXED (2026-08-10) — KI-38 has no loose ends left.** It
read `/proc/<pid>/stat`, the **main thread only**, and a `brood` runtime parks its root thread on
a futex while workers run, so in the reproduction every process printed `S futex_do_wait` and the
`D`/`R`/dead discrimination it exists for collapsed; its `cmd.contains("/brood")` filter also
matched everything under the repo path. It now reads per-thread state from
`/proc/<pid>/task/*/stat`, prints each thread's state char plus the first non-`S` thread's
`wchan` and the process's total CPU ms, and matches argv[0]'s file name. Verified by sabotage
(marker write removed so the wait really times out): two processes listed instead of ~40 lines of
harness, and the child reads as booted-then-parked (15 threads all `S`, CPU flat) — see
`docs/known-issues.md` KI-38.

### 2026-08-07's changes (two commits) — the session before the KI-38 work

**Require edges are part of the image** (KI-37) and **three gaps hatch surfaced** — a `table`
global no longer forfeits the image for the whole project (imaged by value, format v4),
`tcp-read-until`/`tcp-read-n` take `:timeout-ms`/`:max-bytes` so a hardened server can use them,
and `nest format` walks a whitelist (`:format-paths`) instead of the whole root minus an ignore
list (`c3c58843`). Plus a rustfmt fix the pre-push hook caught (`afe4bcff`).

**A GC_STRESS false red was removed from `live_migration`.** Under `BROOD_GC_STRESS` the deep-
receive migration test reds on a *liveness* assertion while the correctness assertion it exists
for passes — collecting at every safepoint means nothing is ever stolen (measured: 400 bursts,
122 s, zero migrations, every per-burst total correct). It now runs 5 bursts and skips the
liveness check under stress only: the sweep goes 120 s + 1 TIMEOUT → **15 s, 37/37**. This matters
because CLAUDE.md instructs you to run that sweep before trusting a green tree, so the trap was
armed for the next person. See §6.

### 2026-08-06's changes (five commits)

**The startup image had never been read from** (KI-34, `34770be4`). ADR-218 shipped it on
2026-08-06 and it was written on every cold start and then ignored: `nest run` restored the root
section and loaded every module from source anyway. Nothing failed — an imaged start behaved
exactly like a cold one, because it *was* one. Two independent defects, either sufficient:
`project-install-image` ran `(def *image-sections* …)` inside module `project` (binding
`project/*image-sections*` while `%require-force`, root code, read the empty root one), and
`%require-force` tested the ADR-070 package branch *before* the image branch — which always
matches, because a project roots its own modules too. Both fixed; `%set-image-source!` is the root
setter, and the image branch now comes first.

**The registry set is derived, not named** (KI-35, same commit). The list of "globals loading
mutates rather than creates" had gone stale three times, always silently — `declared_sigs`, then
seven ability/multimethod registries, then `*method-from*`. `%registry-update!`/`%registry-cas!`
are the only ways a registry is written, so the kernel records those names and `%registry-names`
reports them; what remains in Brood is an *exclusion* list, where forgetting an entry costs a
redundant load rather than a wrong answer.

**Dependencies are imaged too** (`94170dfc`), with their files added to the staleness key — they
live outside `:source-paths`, so nothing else could invalidate them.

**`nest run`'s cold pre-flight was quadratic** (`03efa15a`): 26.8 GB → **5.2 GB** on 16 302
modules, for ~+4% time. `check-project-run-closure` handed every file the same whole-project
reachability list, and a `spawn` deep-copies its captured chunk — so copies scaled as
files × closure. Shipping it once per chunk fixes it. See §9.

### Measurement state — read before re-deriving anything

- **The loader is linear**: ~130 KB and ~1.6 ms per module, flat 500 → 8 000, image size exactly
  linear; 16 302 modules load and image in **30 s / 2.6 GB**. There is *no* per-module memory
  defect — an earlier "~1.6 MB/module" reading was a `nest run` figure wrongly attributed to the
  loader. Reproduce with `scripts/bench/gen-project.py` + `scripts/bench/image-scale.sh`.
- **ADR-218's headline lazy row reproduces but measured the wrong mechanism.** An entry point
  reaching two of N modules pays about the same to source-load two files as to materialise two
  sections, so 1.30 s looked right while the image was dead. The row that was genuinely broken is
  the **eager** one (`nest test` / `nest check` / LSP): **8.55 s → 1.34 s**, materialise time
  7 012 → 959 ms.
- **An instrument that reads healthy either way is worse than none.** The eager path reported
  "materialised 4002 of 4003 sections" in *both* arms — it counts sections walked, not sections
  served. What distinguishes them is a top-level `println` in a module (absent on an imaged
  start) and `BROOD_IMAGE_TRACE=1`, which prints one `[image-section]` line per module actually
  materialised.

---

## 1. START HERE — `spawn-live`: measure the candidate before you build it

**This section has now named the wrong lever four sessions running, and each time the mechanism
it named was the one *nearest the symptom* rather than the one carrying the cost.** Per-process
inline caches, then park/resume, then an identity-keyed IC, then "reach native code", and now
**the receive machinery itself** — five candidates, five refutations, each disposed of by one
measurement that cost far less than the implementation would have. §4 has the first four; the
fifth is item 1 below, retired 2026-08-10 at ~1.8% of the row. **So: before implementing
anything below, measure that its premise still holds.** Everything here is an argument, not a
fact, until re-measured; the facts are the ladder table and §4.

**What has actually been fixed on this row.** `fold` coerced with `seq` and walked with
`first`/`rest`, and `(rest v)` on a vector *materialises a list of the tail* — 15 cons cells and
~48 one-arg primitive dispatches to sum a 16-element payload. `fold` now indexes a vector
directly (`fold--vec`). Per unit: allocations **27.7 → 15.0**, one-arg dispatches **48 → 1.1**,
the shape **32.7 → 28.6 µs** CPU, the published row **11.44 → 10.40 CPU·s** (−9%). And ADR-215
fixed per-process recompilation (compiles 100 154 → 163 per 100k processes).

**Then, 2026-08-09: `%vector-reduce` — the vector fold moved into a native counted loop, −19%
CPU on the row.** `fold-vec` had dropped the list materialisation but still paid a Brood-level
`apply` per element, and — the part that dominated — a reducer like `+` is a thin **passthrough
wrapper** whose redirect was re-resolved on every element. `reduce_prim_op` already resolves that
wrapper once, which is why `(fold + 0 (range n))` is fast, but **nothing on the vector path ever
consulted it**: it was wired only into the two range paths. `%vector-reduce` is the vector
counterpart of `%range-reduce` (tight i64 loop → resolved-once HOF arm → general apply), and the
prelude's vector branch now calls it. A/B on `bench/brood/spawn-live.blsp` at `BENCH_N=300000`,
alternating on an idle box, identical checksums: **9.96 → 8.04 CPU·s (−19.3%)**, wall 2.00 → 1.90.
RSS went 1.60 → 1.63 GB, a ~2% rise that is at the edge of run-to-run noise but is the one number
that moved the wrong way — worth a look if the RSS gap to the BEAM is ever the target.

**Then, 2026-08-10: `fold`'s own dispatch chain was costing more than the fold.** With the
native reduce in, the payload rung was decomposed by isolating one thing at a time on the
spawn-live shape (100k fresh processes, 16-cell payload, median of 3, µs/unit CPU):

| variant | µs | what it adds |
|---|---|---|
| `noop` (no fold) | 20.2 | — |
| `(%vector-reduce %add 0 p)` | 22.5 | **+2.3** the native reduce itself |
| `(%vector-reduce + 0 p)` | 23.9 | +1.4 resolving `+` through its passthrough arm |
| `(fold %add 0 p)` | 27.9 | **+5.4** `fold`'s dispatch chain |
| `(fold + 0 p)` | 28.3 | +4.4 |

So the arithmetic is nanoseconds and the *dispatch* was microseconds. A vector was tested
**last** (`range?` → `seqview?` → `type-of` + `%eq`, four native calls in a cold process)
while a range was tested first. Testing a vector first with the one-call `vector?` predicate
is pure reordering — the branches are disjoint, since `type-of` is `:vector` for a vector and
`:pair` for a range, a seq-view *and* a list — and **nothing regresses**: 300k small folds,
containers hoisted, median of 3, **vector 533 → 410 ns, range 230 → 183, list 3595 → 3248**.
The range row improves even though a range now pays an extra `vector?` first, so flattening
the chain is worth more than the check costs. A/B on the published row: **8.30 → 7.92 CPU·s
(−4.6%)**, and **9.96 → 7.92 (−20.5%) cumulative** with `%vector-reduce`.

**The next lever on this rung, with its size already measured: make `fold` itself native.**
After the reorder, `(fold + 0 p)` is 27.1 µs against `(%vector-reduce + 0 p)` at 23.9 — so
**~3.2 µs/unit is the cold call into the Brood-level `fold` wrapper itself**, not its
predicates (now one) and not the reduce (2.3). That is the largest single remaining piece of
the payload step. It is a real change, not a reorder: `fold` must keep map-as-pairs, the
seq-view fusion (which calls a Brood transducer back), and the exact error/promotion
behaviour. Size it against that ~3.2 µs before starting.

**Where the row now stands.** The ladder is now committed as
`scripts/fuzz/stress/spawn_live_ladder.blsp` (it was `sl_one.blsp`, uncommitted, so these
figures could not be re-derived). Run **one rung per process** and read CPU, not wall:

| rung | µs/unit (2026-08-06) | µs/unit (2026-08-10) | adds, now |
|---|---|---|---|
| `spawn` — spawn + exit | 6.9 | 7.8 | — |
| `send` — + an unread message | 7.1 | 7.3 | ~0 |
| `nopark` — + a `receive` that never suspends | 15.8 | 15.5 | **+8.2** |
| `park-batched` — + every unit suspends (100 coexist) | 17.6 | 14.0 | — |
| `park` — + all N coexist | 22.2 | 21.6 | **+7.6** vs batched |
| `payload` — + the 16-cell copy and fold | 31.5 | 28.9 | **+7.3** |

The `payload` rung fell 33.2 → 28.9 on 2026-08-10 (`%vector-reduce` + the dispatch reorder);
the rest is invocation drift, ~10% between runs, so read the steps and not the levels.

The three big steps are the **receive machinery** (+8.2, but see item 1 — its addressable
part measured ~0), **coexistence** (+7.6) and the **payload copy and fold**
(+9.3), and **coexistence** (+4.6). Suspending is not one of them (item 4 below). The
earlier version of this table read 8.6 / 12.2 / 17.7 / 28.6 for the comparable rungs;
absolutes drift ~10% between invocations, so read the steps, not the levels.

**Where to actually start, as of 2026-08-10.** Items 1 and 2 below have both been measured
since they were written, and the order has changed:

- **Item 1 (receive) is retired for this row** — ~1.8%, inside the noise.
- **Item 2 (payload) is largely banked** — `%vector-reduce` and the dispatch reorder took the
  rung 33.2 → 28.9 µs/unit and the published row −16% CPU. What is left of it is ~2.9 µs for
  the cold call into `fold`, i.e. nativising `fold` (~11% of the row, real regression surface —
  `seq` is not a Rust builtin, so two of its four paths re-enter Brood).
- **The unmeasured candidate that may beat both**: a bare Brood-level call in a freshly spawned
  process costs **~0.85 µs** (measured by inserting trivial forwarders — 23.7 → 24.3 → 25.4
  µs/unit for zero, one and two extra calls). Every prelude call in this row pays that, not
  just `fold`'s. Making a first-call-in-a-process cheap would pay out far more broadly than
  nativising one function. Nobody has looked at where that 0.85 µs goes.
- **Item 3 (coexistence, +7.6 µs/unit)** is the other big step and is untouched by all four
  wins — the ~5.5 KB live-process floor, which the published row's flat ~1.6 GB RSS reflects.

The original list follows, corrected in place:

1. **The receive machinery (+8.2 µs/unit) — RETIRED FOR THIS ROW 2026-08-10, worth ~1.8%.
   Do not start here; read the correction below before anything else in this item.** The step
   is real and the matcher really does miss the native fast frame, but making it native was
   measured directly on this row and buys almost nothing. Its "caching" premise was separately
   REFUTED 2026-08-06. Kept in full because the mechanism is worth knowing and may matter on a
   long-lived message row. Historically it was described as the widest
   reach of anything on this list — every message-passing row pays it (`latency`, `pingpong`,
   `supervisor`), not just HOF loops — but the mechanism is not the one this item named.

   **The premise was wrong.** `receive` is a *macro*: `%match-build-from` lowers the clause set
   at macro-expansion time into a literal `(fn (msg) …)` whose body is a fully inlined
   `vector?`/`vector-length`/`vector-ref`/`%eq` if-tree. Dump it —
   `(println (macroexpand '(receive ([:go v] 1))))` — and there is no clause compiler left to
   run. `%match-compile-clause` executes **once per site, at load**, never per message; the ~20
   `match-*`/`receive-*` arms seen tiering during the ladder are that expansion work, not
   per-message work. So there is no ADR-215-shaped cache to add here. Compiles are already flat
   (`BROOD_TRACE_COMPILE`: 169 vs 179 across rungs at N=2000), and `nopark` adds **no**
   per-unit promotion over `send` (`BROOD_TRACE_PROMOTE`).

   **What the cost actually is.** `hof_resolve` succeeds on **100%** of receives and
   `hof_apply_step` is entered on 100% — but `hof_apply_native` declines every time, so every
   matcher call pays the `vm_apply` → `vm_run_bc` trampoline. The matcher arm lowers to native,
   then **deopts on every activation**; where `deopt_watch` is set, sixteen in a row make
   `jit_deopt_feedback` mark it `BAILED` for the rest of the program
   (`DEOPT_BAIL_CONSECUTIVE = 16`). Same failure shape as the float-global bug in `CLAUDE.md`.
   It is *by design* as a self-heal — closure arms are deliberately exempt from the static
   call-mediated profitability gate so deopt feedback can judge them at runtime — but here the
   arm is being judged for something it should pass.

   **WHICH patterns pay it, bisected 2026-08-06** (`scripts/fuzz/stress/recv_matcher.blsp`,
   one long-lived process so tiering is not a variable; N=300k, perf-stats build):

   | `PAT` | pattern | compares a literal? | ns/iter | `jit_deopt` | `jit_link_done` |
   |---|---|---|---|---|---|
   | `any` | `m` | no | 1266 | 0 | 281 043 |
   | `bind` | `[a v]` | no | **1286** | 0 | 281 812 |
   | `vec` | `[:go v]` | yes (keyword) | **1776** | 281 978 | **0** |
   | `intlit` | `[1 v]` | yes (int) | 1763 | 281 467 | **0** |

   **It is comparing a literal element that costs the native frame** — and `bind` is the honest
   control, because it does the *same* vector work as `vec` (same `vector?`, same
   `vector-length`, two `vector-ref`s) and merely binds the head instead of comparing it. So
   the gap is **−28% of the whole receive loop**, not an artefact of work not done. (The
   earlier catch-all comparison put `ns_match_run` at 534 vs 112 ns and `ns_receive` at 935 vs
   485; treat those as the same story measured against a weaker control.)

   That also rules out three things at once: it is **not** the matcher's calls (`bind` calls
   `vector?` and `vector-length` and stays native), **not** the vector machinery, and **not**
   keyword-specific (an int literal deopts identically). None of `BROOD_NO_LEAF_INLINE`,
   `BROOD_NO_INLINE`, `BROOD_NO_PARTIAL_LEAF` or `BROOD_LINMAP=0` move it either — it is in the
   base lowering. `BROOD_DEOPT_TRACE=1` names the deopting arm as the matcher `<closure>`
   itself (`watch=false resume_ip=-1`).

   **Reach: this is the entire tagged-tuple idiom** — `[:go v]`, `[:reply ^r v]`, every
   supervisor and `gen` protocol message. Bind-only patterns are the rare case; essentially
   every real `receive` is on the slow side of this line.

   **MEASURED 2026-08-10 AND DECLINED FOR THIS ROW — fixing the matcher is worth ~0 on
   `spawn-live`.** Before building anything here, the counterfactual was run directly: the
   ladder's unit with three receive patterns doing identical work, interleaved, 3 rounds,
   µs/unit CPU — `[:go p]` (bails) **28.0**, `[t p] :when (%eq t :go)` (also bails) **27.8**,
   `[_t p]` (stays native) **27.5**. That is ~0.5 µs/unit, ~1.8%, with overlapping spreads.
   The guard row is the control that makes it readable: it bails like the tag row, so the
   small gap is the native frame and not the skipped comparison.

   This matters more than the `recv_matcher` figures suggested, because the arm is shared
   process-wide (ADR-215) — 16 deopts anywhere bail it for all 100k units — and it *still*
   buys nothing. So the receive step (+8.2 µs/unit) is real but its cost is **not** the
   native frame; it is the rest of `receive` (mailbox scan, delivery, message copy). **Do not
   spend a session on the deopt for this row's sake.** It may still be worth something on a
   long-lived message-passing row (`latency`, `pingpong`, `supervisor`) — that is untested,
   and is the only remaining reason to look at it.

   **CORRECTION 2026-08-09 — it does NOT deopt per activation; it deopts 16 times, then is
   latched off for the run.** The counters said `jit_deopt 281 978` and this item read that as
   "deopts on every activation". It is not: `jit_deopt` counts `jit_tier`'s outcome-1 only and
   is **blind to every deopt taken on the HOF fast frame**, which is the path the matcher uses.
   With a counter added on that path (`hof_native_deopt`) the real shape at N=300k is
   **`jit_link_done` 16 · `hof_native_deopt` 16 · `hof_decline_bailed` 283 961**: the arm links,
   deopts 16 consecutive times, `jit_deopt_feedback` latches it `BAILED`, and every one of the
   remaining ~284k calls is then *declined before it runs* — no native attempt, no deopt.

   So the recurring puzzle in this item — "with deopts eliminated it still does not link" — has
   a mundane answer: `BAILED` is sticky for the process, so removing the deopt *source* does not
   un-bail an arm that already bailed. And the per-call cost being paid is `vm_apply`, not a
   deopt round trip.

   New counters for this, all `perf-stats` only and free otherwise: `hof_decline_nocode` /
   `_bailed` / `_queued` / `_depth` / `_epoch` say **why** `hof_apply_native` declined (they sum
   to the declines), and `hof_native_deopt` counts the frame's own deopts. `jit_link_done`
   reading 0 against N calls now names its own cause in one run instead of needing a bisect.

   **Read `hof_decline_*` before theorising about the 18 CLIF edges.** The instrument this item
   proposed (stamp a per-site id into the deopt block) answers a *different* question — which
   guard fails — and is only worth building if the 16 deopts turn out to matter, which at 16
   occurrences they almost certainly do not. The cost is the 284k declines.

   **GO/NO-GO, 2026-08-06 (read this before implementing anything here).** The deopt is real and
   its guard is now known — `eq_dispatch`'s non-interned fallthrough in `jit_lower/emit.rs`;
   routing it away from `f.deopt` takes the arm's deopts from ~282k to **0**, with the arm still
   lowering. But **removing it is worth ~0–5%, inside the noise**: clean `--release`, N=300k,
   three runs each, `vec` medians **1290 ns/iter baseline vs 1226 deopt-removed**, spreads
   1230–1330 and 1203–1436. It also does **not** restore the native fast frame —
   `jit_link_done` stays 0 — so the deopt is not why the matcher misses it. A general
   non-deopting `=` fallback was therefore designed and **not built**. Two consequences: the
   `vec`-vs-`bind` gap is **~17% on a clean build**, not the 28% first quoted (that came from a
   perf-stats build, whose counters perturb, against `any` rather than `bind`); and part of even
   that 17% is `bind` doing *different* work (binding the head and building a 3-element result
   against comparing the head and building a 2-element one), not pure overhead. What survives is
   the counter-verified structural fact — `vec`'s matcher takes `vm_apply` on ~100% of calls
   while `bind`'s takes the fast frame — and an unexplained one: with deopts eliminated it
   *still* does not link. That is the question to answer before any more work here.

   **The deopt's shape, kept because it cost a session to establish: the matcher deopts
   iff it has a SECOND conditional exit.** `[a v]` compiles to one guard (the
   `vector?`+length test) followed by a straight-line bind chain, and is native. Everything
   that adds a further branch-to-`nil` inside that chain deopts, and all three variants
   measure the same (~290k deopts / `jit_link_done` 0 at N=300k):

   | shape | | deopts? |
   |---|---|---|
   | `[:go v]` | literal compared inside the pattern | yes |
   | `[1 v]` | int literal instead of a keyword | yes |
   | `[v :go]` | literal in the *second* position | yes |
   | `[a v] :when (%eq a :go)` | same compare as a clause **guard**, after the binds | yes |
   | `[a v]` / `m` | no second exit | **no** |

   So it is not the comparison, not its operand type, not its position, and not
   pattern-vs-guard — those are all the same thing to the lowering. It is the extra
   conditional exit. (Chunk tell: the deopting matchers end `… MakeVector Jump Const Jump
   Const` — two `nil` exits — against `bind`'s single trailing `Const`.)

   **What the CLIF says so far, so it need not be re-read.** Extract the matcher with
   `BROOD_JIT_DUMP_IR=1` and select the `<closure>` arm containing `MakeVector` (there are
   several `<closure>` arms; the first is a prelude closure — and at low N the matcher may not
   have tiered yet, so use N≥300k or its *absence* will mislead you). `vec` has 18 edges into
   the deopt block against `bind`'s 15; the extra three are exactly one additional
   `eq_dispatch`. Both `eq_dispatch` sites read **correct**: int×int compares payloads and
   either-side-Sym/Keyword (tags 5/6) compares interned ids, so for `(%eq el :go)` — keyword
   against keyword — control provably reaches the non-deopt block. **The failing guard is
   therefore one of the other 16 edges, not the equality.** Ignore the deopt block's one
   *unconditional* predecessor: that is the stack-headroom prologue.

   **Next step, and it should be an instrument rather than more reading:** the 18 edges are
   indistinguishable at runtime because they all jump to one shared deopt block. Give each
   guard site its own identifiable exit — e.g. have `jit_lower` stamp a per-site id into a
   heap field on the way out — and the counter names the guard in one run. Do NOT navigate by
   the deopt's `resume_ip`: it names the nearest checkpoint, not the failing guard (§6).
2. **The payload step — the copy is NOT the cost, and neither is what this item used to
   blame. RE-MEASURED 2026-08-09; two of its claims were wrong.**

   **(a) "Hand-rolling the sum as an indexed loop recovers ~10.7 µs" is FALSE in this regime.**
   Measured on the spawn-live shape itself (100k fresh processes, 16-cell payload, one variant
   per process), µs/unit CPU: `nopayload` 21.0 · `nofold` 20.4 · `fold %add` 29.2 ·
   `fold +` 34.0 · **hand indexed loop 64.1**. The hand loop is **2× worse**, not 10.7 µs
   better — a Brood-level loop in a *cold, short-lived* process is interpreted, where the
   native `fold` builtin is not. The old figure came from warm, long-lived-process
   measurements (`fold +` 163 ns/elem ⇒ ~2.6 µs for 16 cells, which could never have
   accounted for a 9–13 µs step in the first place — the arithmetic did not close, and that
   was the tell).

   **(b) The dominant cost was the passthrough reducer, and it is now fixed.** `fold +` vs
   `fold %add` was **4.8 µs/unit** — pure wrapper redirect, re-resolved per element, on a row
   whose total is ~34 µs. `%vector-reduce` (see the top of this section) closed it: the gap is
   now 1.6 µs and the whole rung went 33.2 → 28.9 µs/unit. **`nofold` ≈ `nopayload` confirms
   the 16-cell copy costs ~0**, which the old `ns_msg_in` reading already said.

   What remains of the step after the fix is ~8.8 µs/unit of genuine per-element callback work,
   and *that* is what the speculative-inline lever below would attack. Original text follows.

   **The payload step (+9.3 µs/unit) — and the copy is NOT the cost.** `ns_msg_in` *fell* (216 →
   180 ns/unit) when the message grew from `[:go]` to a 16-element vector, so the deep copy this
   row was built to measure costs ~0.2 µs of a 31 µs row. Of the +12.5 µs the rung adds under
   perf-stats, only ~1.3 µs (10%) lands in any runtime timer; the rest is Brood-level `fold`
   work, which corroborates the older finding that hand-rolling the sum as an indexed loop
   recovers ~10.7 µs.

   Per element, warm: **hand loop 10 ns · `fold %add` 78 · `fold +` 163 · `fold myadd` 231**. Even
   the best HOF case is ~7× an inlined op. The wrapper's ~85 ns goes to `passthrough_arm` (closure
   deref + `select_arm` + a `SmallVec` **clone** of the arg map), two thread-local ticks in
   `passthrough_redirect_ok`, a fresh argv `SmallVec`, then `call_native`'s checks — five small
   costs, none dominant. **Measured and reverted:** memoising the redirect target on the arm is
   worth **2%** (167 → 163 ns); don't re-try it.

   **The only lever left for this shape is not calling per element** — an identity-guarded
   speculative inline of the HOF's step closure. Groundwork is further along than FRONTIER's
   "true call inlining" bullet suggests: ADR-210 already splices *statically known* leaf callees
   with a deopt checkpoint, and the missing piece is a guard on the step closure's identity. This
   is the one candidate in the neighbourhood that changes the *shape* rather than a constant —
   and note that the JIT gives this shape nothing today (a lowered `loop-computed` measures 274
   ns/call with the JIT and 271 without), so inlining is the whole prize.
3. **Coexistence (+4.6 µs/unit) — the cost of a live *idle* process.** 22.2 µs/unit with 100k
   alive vs 17.6 with 100, same parking either way. This is the ~4.27 KB floor plus the GC/cache
   pressure of 100k live heaps. §4 records that attacking the floor by boxing `Heap` in `Process`
   is the wrong trade (`spawn` +3.2%, `spawn-live` +6.4%) and that the direction is cutting the
   *number* of allocations per process, measuring the `spawn`/`spawn-live` pair alongside the
   floor from the start.
4. ~~**Park/resume (5.5 µs/unit)**~~ — **measured 2026-08-05: suspend/resume costs ~0, and
   this item was a confound.** The rung that produced 5.5 µs changes **two** things against
   the one below it, which its own wording admits ("+ every unit held alive, *so* each
   parks"): every unit suspends, *and* all N are alive simultaneously. Separating them with
   `scripts/fuzz/stress/spawn_live_ladder.blsp`'s `park-batched` mode — units still park, but
   only `BATCH` coexist, verified by `BROOD_L1_STATS` showing one parked-receiver hit per
   unit — at `BATCH=1000`, five interleaved runs each, best CPU ms per run:

   | | runs | median |
   |---|---|---|
   | `nopark` (never suspends) | 1589 · 1710 · 1599 · 1589 · 1609 | 1599 |
   | `park-batched` (every unit suspends) | 1560 · 1560 · 1560 · 1540 · 1589 | **1560** |

   Parking is **2.5% cheaper**, not 5.5 µs dearer — consistent with ADR-178, whose local-send
   fast path fires *only* on a parked receiver, so suspending puts the wake on the fast path.
   The whole delta is **coexistence**: `park` (100k alive) 22.2 µs/unit vs `park-batched`
   (100 alive) 17.6 vs `nopark` 15.8.

   **So the lever is the cost of a live idle process, not the parking mechanism** — the
   ~4.27 KB floor and the GC/cache pressure of 100k live heaps. §4 already records that
   attacking the floor by boxing `Heap` in `Process` is the wrong trade, and that the
   direction is cutting the *number* of allocations per process.

   **Two traps this measurement walked into, both worth keeping.** The batch curve is
   **U-shaped**, not monotonic (BATCH 10 → 19.9 µs, 100 → 17.7, 1000 → 15.7, 10000 → 17.7,
   all → 22.5): a small batch serialises on the parent (spawn K, release K, collect K, ×N/K)
   and reads as a *high* per-unit cost that has nothing to do with coexistence, so picking
   the endpoints of that curve would have "confirmed" either story. And the ladder must be
   run **one rung per process**: in a single process the later rungs inherit the earlier
   ones' JIT tiering, which put `payload` *below* `park` and `send` below `spawn` — a
   monotonicity violation that is the signal the run is contaminated.
5. **Per-process inline caches (~2 µs)** — real but small, and now correctly sized. If you do it,
   the site-id work is already done; what is left is the race design for a shared block. Note the
   identity-IC result in §4 before assuming an IC buys anything here: on the VM, a cached callee
   measured *slower* than resolving one.

Measure with **CPU time over a fixed unit count, binaries interleaved** (<2% spread). The 20.6%
"noise floor" this row was once credited with is an artefact of measuring *wall* on a 3.3-core
workload. `BROOD_PERF_STATS=1` on a `--features perf-stats` build gives `ns_*` timing shares
(`ns_quantum` nests the rest) and per-unit counters; `BROOD_TRACE_COMPILE=1` names every compile.

## 2. Then: rope-native structural motion, the editor half of the `sexp` story

ADR-214 made a *sequence* of structural motions linear when the caller holds one text value —
the tooling/LSP shape. The **editor** shape is untouched and cannot be fixed by that cache:
`sexp/forward` and friends call `(buffer-text buf)`, which is `rope->string`, so every motion
allocates a fresh O(n) string before any scanning happens, and the safepoint table can never
hit. A keystroke-driven motion is therefore O(buffer) no matter how fast the scan is.

The options, in the order I would try them:

- **Motions over the rope.** `std/tool/sexp.blsp` is written against `(text point) -> point`
  with `text` a string; `parse-source-positioned` and `scan-form-start` both take strings. A
  rope-native path needs at least a rope-shaped form-start scan (ropey exposes chunk iteration,
  so the same safepoint idea applies) and a decision about whether the CST walk slices a rope
  or a string.
- **Or: let the caller hold the string.** A command loop that keeps one `text` value across a
  run of motions gets ADR-214's cache for free. Cheap, and it only helps a *sequence* of
  motions between edits — worth checking against how the downstream editor actually drives it
  before building anything in the kernel.

Measured 2026-08-07, and it settles which option: the stringify does **not** dominate. The
`rope->string` copy is ~14% of a buffer-path motion; the dominant residual is the *table miss*
— a fresh string value per motion means `scan-form-start-2` re-scans O(pos) from the top every
keystroke, so the motion stays O(buffer) even though the window END is now native
(`scan-form-end`, below) and the held-text path is flat. So "let the caller hold the string"
does not help while `buffer-text` mints a new value each call: the fix has to make the *cache*
survive across motions — rope-native scanning, or a GC-safe `rope->string` memo (an unchanged
rope yielding the *same* string value, so both the ADR-213 index and the ADR-214 safepoint
table hit). The rope-native scan is the more direct of the two.

(The window END of `narrow` — the forward ~3-form scan — was itself an interpreted `char-at`
loop measuring ~85% of every motion; it is now the native `scan-form-end` (devlog 2026-08-07).
That closed the last interpreted loop on the *tooling* shape but changes nothing for the editor
path above, exactly because that path is dominated by the table miss, not the scan.)

## 3. Then: the `expect_string` copy seam, by body cost

`expect_string` returns an **owned** `String` at ~105 remaining call sites — one copy of the
argument per call. Eight were converted on 2026-08-04, and the measurement gives the rule:

- the copy is worth removing where the call's own work is **O(1)** and the argument is a whole
  buffer — `(grapheme-at txt 0)` on 212 KB measured **−74%**;
- it is **noise** where the body is per-char — `grapheme-count` / `string->codepoints` over the
  whole string measured −0.6%, inside the drift floor, because UAX #29 segmentation dwarfs a
  memcpy.

So triage by what the body costs. Two sites **cannot** take the borrow at all: `string-split`
and `scan-tokens` allocate per piece *while* scanning, and `string-split` would gain nothing
anyway (its copies are the parts, not the input). `scan-tokens` would need a two-pass rewrite
(collect ranges, then allocate) — worth it only if the fontifier shows up in a measurement.

## 4. Closed — do NOT re-attempt these

Each was measured to a conclusion. Re-deriving them costs a session each.

- **Three explanations for the receive matcher's deopt** — all measured and **killed
  2026-08-06**, in the order they were tried. Each cost a build; re-deriving them costs the
  session. The symptom under test throughout: the matcher arm deopts 16× and is marked
  `BAILED`, so every receive pays the `vm_apply` trampoline (§1 item 1).

  1. **"It is the non-tail calls (`vector?` / `vector-length`) — make them prims."** No. The
     deopt's `resume_ip` sat immediately after a call, which reads as causal. It is not: the
     `resume_ip` names the nearest *checkpoint*, and checkpoints sit after calls. Implemented
     `PrimOp1::VectorLen` (IR + both VM exec paths + a Cranelift `inline_vec_len` mirroring
     `inline_vec_ref`) — the chunk's `Call` duly became `Prim1`, and the deopt simply **moved
     to the other call**. Removing that one too (the matcher generator emitting
     `(%eq (type-of t) :vector)` instead of `(vector? t)`) left a **call-free** matcher that
     deopts on **every** activation with no checkpoint at all (`resume_ip = -1`) and no longer
     self-heals — measurably **worse** (1936 vs 1627 ns/iter, same build), because losing the
     `BAILED` latch means paying a failed native entry per call forever. **Reverted; nothing
     of it is in-tree.** If a later thread wants the `VectorLen` prim for its own sake, it is
     ~120 lines and mirrors `inline_vec_ref` (`jit_lower/emit.rs`) exactly — but it must be
     justified on its own measurement, because on this row it bought nothing.
  2. **"The message vector is not LOCAL, so the inline vector ops deopt on the region check."**
     No — `inline_vec_ref`/`inline_vec_len` do deopt on a non-LOCAL handle, but instrumenting
     the deopt to print the operand's region gave **`region=0` (LOCAL) on every one** of 42 121
     deopts.
  3. **"The clauses are re-parsed/re-compiled per execution"** — the original §1 premise. No;
     see §1. `receive` is a macro and the matcher is an inlined if-tree.

  Two things worth keeping from that hunt. **`jit_link_done` is the counter that answers "did
  the HOF fast frame engage?"** — 0 against ~N calls is the whole diagnosis, and no other
  counter shows it (`jit_native` stays high because the *caller* is native). And
  **`BROOD_NO_HOF_JIT=1` measuring flat is not evidence the native path is worthless** — here
  it measured flat because the path was never taken at all.

- **"Make a short-lived process reach native code"** — measured and **declined 2026-08-05**,
  before being built. The premise came from `spawn-live` gaining *nothing* from the JIT
  (`BROOD_NO_JIT=1`: payload rung 4310 → 4280 ms, park rung 2050 → 2050) while
  `BROOD_JIT_DUMP_IR` shows 171 arms lowering — which reads as "the native code is compiled
  and the short-lived units never get to it", i.e. an ADR-215-shaped hole (share the *tier
  decision* the way ADR-215 shared the *code*).

  Both halves are wrong. **The units' own arms do lower** — `fold` and `fold-vec` each appear
  in the dump (twice, the two-stage dual body), as do `receive`, the `match-*` family, and
  `<closure>`. And **native is not faster for the shape that dominates**: `hof_call.blsp`'s
  `loop-computed` lowers (confirmed in the dump) and still measures 274 ns/call with the JIT
  and 271 without. A HOF-call-dominated loop pays for the *call*, which re-enters the runtime
  either way, so there is no native win to reach for.

  **What it converges on instead:** the only lever left for this shape is not calling per
  element — the identity-guarded speculative inline of the step closure that §1 named all
  along. Everything else in the neighbourhood has now been measured and declined: the IC
  below, park/resume, and this.

- **An identity-keyed call-site IC for a computed (local) callee** — measured and
  **declined 2026-08-05**, before being built. §1 recommended it because `compile_node`
  allocates an IC id only for a free-global head, so a HOF's step call re-resolves per
  element. The premise is real and the *cost* is not: with `scripts/fuzz/stress/hof_call.blsp`
  (3M calls, callee reached as a global vs as a parameter, same callee, **same arity**, a
  non-inlinable body so the comparison is a call and not a splice) —

  | | JIT on | JIT off |
  |---|---|---|
  | global head (IC + fast-link) | 242 · 227 · 245 ns | 247 · 246 · 276 ns |
  | computed head (no IC) | 263 · 248 · 267 ns | **237 · 239 · 266 ns** |

  On the VM the computed callee is *faster* — three runs each way — because the global path
  pays an IC probe and validation while the computed path just reads a slot. So the thing
  the IC would cache (`passthrough_arm` probe + `compiled_arm_for`) costs about nothing to
  recompute, and the ~21 ns (8%) gap that does appear under the JIT is the **native
  fast-link**, not the cache; capturing that needs an identity-keyed `FastLink` slot, which
  is KI-20 territory, for 8%.

  **Checked short AND long, because a tiered runtime has two steady states** (the rule is
  now in `CLAUDE.md`). Sweeping the call count over four orders of magnitude, gap =
  computed − global:

  | N | 10k | 100k | 1M | 10M |
  |---|---|---|---|---|
  | warm gap | +18 ns | +23 | +19 | +23 |
  | cold gap (single pass, no warm-up) | **−63 ns** | +17 | +18 | +22 |

  The warm gap is flat at 7–8% from 10k to 10M, so the verdict is not an artefact of a
  half-tiered arm — and `BROOD_JIT_DUMP_IR` counts the **same 28 arms lowered** at 10k and
  1M with zero deopts, i.e. tiering is already complete by ten thousand calls. The cold
  column makes the case *against* the IC stronger, not weaker: on a short run the **global**
  arm is 63 ns/call worse, because it is the one paying IC install and tiering cost. An IC
  would be worth ~0 for long-lived work and negative for the short-lived kind.

  **The trap that nearly sold it, worth keeping:** the first version of that benchmark used
  `(defn step (acc x) (%add acc x))` as the callee and measured the global head at **1
  ns/call against the computed head's 160** — an apparent 160× that reads as a screaming
  case for the IC. That shape is a *passthrough to a `%`-native*, which `resolve_prim`
  (`compile/mod.rs:668`) inlines to a `Prim2` at the call site, so the row was measuring a
  deleted call. A callee is only measuring a *call* if it cannot be inlined — and a row
  reporting ~1 ns/call is reporting that its work is gone, which is why the committed
  version prints total ms and the accumulator beside every figure.

- **`spawn-live`'s per-process recompilation** — **fixed** (ADR-215): the compiled-code cache
  was keyed by the closure *handle*, and a no-capture closure is promoted afresh per creation
  (ADR-194), so every `spawn` thunk and `receive` matcher missed and every process recompiled —
  100 154 compiles per 100 000 units at 8.1 µs. Keyed by AST now. Do not re-attempt the
  *mechanism* (ADR-175 shipped it correctly); the bug was the key.
- **Nine scheduler/messaging switches as an explanation for `spawn-live`** — all measured
  neutral on it (`NO_HANDOFF`, `NO_STEAL_WAKE`, `SPAWN_RR`, `SPAWN_SPILL`, `NO_RECV_MARK`,
  `NO_JIT`, `MIMALLOC_PURGE_DELAY=0`, `NO_SHARE_FN`). The row is not a scheduling problem.
- **`sexp motions`, the last quadratic in the sweep** — **fixed** (ADR-214): the form-start scan
  resumes from a safepoint table cached against the string value, and runs over bytes instead of
  a per-call `Vec<char>`. Ratio 7.97 → 3.88, 18570 → 6146 ms at 12800 forms, linear on four
  rising bases. Do not re-attempt it as a *constant-factor* fix: two of those landed first and
  left the shape untouched. What is left is the **editor** path (§2), which this cannot reach.
- **`sse--frames`** — **fixed**: one `string-split` instead of a `substring` of the rest per
  event, 5671 → 15 ms at 25600 events, with its own sweep row.
- **`editor/lineedit`'s per-keystroke geometry** — measured and **declined**: 2.5 ns/char, so
  28 µs at 10 K chars and 2452 µs at 1 M. Under a frame even on a 1 MB pasted line.
- **Char→byte conversion for non-ASCII strings** — **fixed** (ADR-213): a sparse char→byte index on
  the string slot, so a char index costs a lookup plus a walk bounded by 32 chars in either
  encoding regime. `inc-scan` 16.85× → linear; `sexp motions` lost its 9.80× → 5.42× encoding
  penalty. Do not re-apply the *char-count cache* idea expecting more: its mechanism is the ASCII
  test itself, which is why it never reached the slow path.
- **Forcing `mandelbrot`'s `row-sum` onto the native path.** Refused by the call-mediated
  profitability gate in `jit_lower_arm`; exempting it makes `mandelbrot` **+0.7%** and `matmul`
  **+5.1%** (0.3% floors). The arm *does* lower under the exemption — it simply is not faster. The
  real `mandelbrot` lever is removing the boxing (unboxed floats across call boundaries), a much
  larger piece of work.
- **Boxing the `Heap` inside `Process`** (memory-floor experiment (a)). `Process` 1304 → 112 bytes
  and the floor fell 4273 → 4124 B/proc (−3.5%), but it adds a second allocation per spawn:
  `spawn` **+3.2%**, `spawn-live` **+6.4%**. Wrong trade; reverted.
- **The allocator size-class histogram** (experiment (b)) — retired by (a)'s shape: `Process` shrank
  1192 bytes of struct and the floor moved 149, so size-class rounding is not the dominant term. The
  floor is spread thin across ~25–30 blocks. If anyone wants that row, the direction is cutting the
  *number* of allocations per process, not their sizes — and (a) shows that pulls against `spawn`,
  so measure the `spawn`/`spawn-live` pair alongside the floor from the start.
- A memory leak (chased, does not exist); endurance (16/16 soak, 12.7 M iterations); thread 6's
  throughput decay (fixed, ADR-208); the RUNTIME reclamation threshold (dissolved); per-message cost
  as an explanation for `latency` (it was spawn placement — send+receive is 1.1 µs).

## 5. What shipped recently, and how to turn each piece off

Every mechanism has an off-switch, because each is an optimisation whose fallback is the old
behaviour. **If something misbehaves, bisect with these before bisecting commits** — and for a
mechanism with a switch, the switch on ONE binary is the attribution (§6).

| Change | Off-switch | Worth |
|---|---|---|
| Vector `fold` in a native counted loop + vector-first dispatch (2026-08-10) | — (no flag; bisect `std/prelude.blsp`'s `fold` and `%vector-reduce` in `builtins/sequences.rs`) | published `spawn-live` **5.55 → 4.66 CPU·s (−16%)**; small folds 533 → 410 ns (vector), 230 → 183 (range), 3595 → 3248 (list) |
| Boot cache warmed once before the suite fans out (KI-38) | `BROOD_NO_WARM_BOOT_CACHE=1` (nextest cannot skip a setup script — `--config 'profile.default.scripts=[]'` does **not** work) | the three boot-wait tests at `-j 64` cold: **FAIL 20.1 s → 2.6 s**, and 4.1/3.7/4.7 s → 0.4/0.5/1.2 s at the default `-j`; costs ~2.4 s once |
| Compiled code keyed by the closure's AST, shared per runtime (ADR-215) | `BROOD_NO_SHARED_ARMS=1` | `spawn-live` wall −12.5%, CPU −25%, RSS −14%; bytecode compiles 100 154 → 163 per 100k processes |
| Form-start safepoint table on the string value (ADR-214) | — (a cache that changes no answer; gated by equality with the pre-table scan at every position) | `sexp motions` 7.97× → 3.88×, 3.0× at 12800 forms |
| Sparse char→byte index on the string slot (ADR-213) | — (a cache that changes no result; its gate is that its answers equal the walk's) | multi-byte char indexing **96×** on a micro (60.2 s → 0.62 s); `inc-scan` 16.85× → linear; `sexp motions` 9.80× → 5.42×; ASCII flat |
| Partial leaf splicing (ADR-210) | `BROOD_NO_PARTIAL_LEAF=1` | 2.4× on a lowering caller with a leaf beside a residual call; every published row flat |
| Shared closure crosses a **serialised** send by handle (ADR-208) | `BROOD_NO_SHARE_FN_MSG=1` | `rt_closures` 143,752 → 66 constant; RSS 213 vs 502 MB |
| Idle peer told at once that a peer queued a child | `BROOD_NO_STEAL_WAKE=1` | `latency` p50 27 → 19 µs, p99 124 → 78 µs |
| Owner keeps first refusal on a fresh child | `BROOD_STEAL_GRACE_NS=<n>` (0 disables) | protects `supervisor`; the cliff is at 2.5 µs on this machine and the default takes a 2× margin deliberately |
| Closure sends share already-shared code, **parked** path (ADR-194) | `BROOD_NO_SHARE_FN=1` | retained closure 436 B → 48 B |
| Spawn placement spills off a backlogged worker | `BROOD_SPAWN_SPILL=999999` / `BROOD_SPAWN_RR=1` | `latency` p50 5×, p99 2.9× |
| Receive-mark (ADR-195) | `BROOD_NO_RECV_MARK=1` | backlogged reply O(backlog) → O(1), 653 → 4 µs at 32k |
| Fast-link deopt shape check is flag-free (KI-26) | — (correctness) | a peer's stale link no longer re-runs a journaled effect from ip 0 |
| Registry updates are atomic (KI-22/23) | — (correctness) | ~40% of concurrent registrations were being lost |

**`std/` quadratics fixed** (each with a `scale_sweep.blsp` row so it cannot come back):
`template/render` 318→24 ms · `last-index-of` 540→1 ms · `strip-ansi` 1583→109 ms ·
`stream-lines` 303→39 ms · `format-source` 3593→1988 ms · **`sexp` motions 12061→6037 ms (2.0×)** ·
**`markdown-spans` multi-byte 1287→559 ms (2.3×)** · **`inc-scan` multi-byte 118→3 ms (ADR-213)** ·
**`sexp motions` 18570→6146 ms, quadratic→linear (ADR-214)** · **`sse frames-1c` 5671→15 ms**.

**Every row in the sweep is now linear in both encoding regimes.** Its job from here is
regression detection, which means running it **both** ways (`UTF8=1`) and checking the ratio
*trend* across bases, not one triple.

## 6. Traps — every one of these cost real time

**Proving green**

- **A red under `BROOD_GC_STRESS` is not automatically a bug — check *which* assertion fired.**
  Collecting at every safepoint changes scheduling, not just speed, so a *liveness* assertion can
  fail while the correctness assertion beside it passes. `live_migration`'s deep-receive test did
  exactly that (400 bursts, 122 s, zero migrations, every total correct) and is now gated on the
  env var. Worse, it took 122 s against nextest's 120 s cap, so the failure was reported as a
  **TIMEOUT** — which tells you nothing about which assert fired. If a stress sweep times a test
  out, run that test standalone with a raised cap before concluding anything.
- **A flake hunt that deletes its logs on success cannot be believed.** A `nextest -E` filter that
  matches nothing exits 0, so "0/25 failures" and "the filter was wrong" look identical. Keep the
  per-iteration logs, or assert on `1 test run` per iteration. Confirm with `cargo nextest list -E`
  that the filter selects what you think it does.
- **Re-running a flaky test *idly* usually just repeats what the entry already records.** KI-36
  already had "0 failures in 25 idle runs"; a second 25 added nothing, because neither reproduced
  the condition of the sighting (a 4000-module image build beside the suite). Before spending the
  runs, check that your load actually reaches the test's path — KI-36's own synthetic-load attempt
  is the counterexample, at 1.6–1.9 s loaded against 2.58 s idle.

**A boot measurement taken on an already-built tree is a WARM measurement.** The
expanded-prelude cache is keyed on each binary's own mtime, so it is cold exactly once per
binary per rebuild — and a cold boot is **~11x** a warm one (1.23 s vs 0.11 s, essentially all
macro-expansion; `BROOD_BOOT_TRACE=1` prints which path a boot took and where the time went).
KI-38 sat undiagnosed for two sessions because 4915 boot samples were taken beside and after
suite runs, i.e. entirely on the warm path, and the deadline was then judged against that
distribution. If you are timing a boot, state which path you measured, and use
`XDG_CACHE_HOME` to isolate a cold one (`scripts/ki38/bootcost.sh` does exactly this).

**Running the suite at all**

- **Do not run two debug `make test` runs at once on this box.** 30 GB, ~420 MB debug binaries,
  ~650 MB per suite process, and `release_bundle` tests at **835 MB RSS each**. Two concurrent
  suites on 2026-08-07 ended with the editor process dying on memory at ~20:56 and taking its own
  suite with it (`TRY 1 TERM [808 s] brood_suite_passes` — children reaped with the parent). The
  three kernel OOM kills earlier that day (26.5–26.8 GB, `brood`/`nest`) were a *different* cause,
  the quadratic pre-flight since fixed in `03efa15a`.
- **Any ad-hoc tool that spawns `brood` children leaks them when *it* is killed — that is KI-29,
  and you will re-create it.** A sampler loop killed with `kill $PID` mid-iteration orphans the
  child it had just spawned, and a `(park)` program never exits: seven orphans accumulated on
  2026-08-07, one per time the loop was stopped, and were found only because an unrelated
  diagnostic listed live `brood` processes. The test harness solves this with
  `support::dies_with_parent` (`PR_SET_PDEATHSIG`); scratch tooling has nothing, so check
  `pgrep -af '/tmp/brood-'` after any session that spawned children in a loop.
- **`brood`/`nest` panics land in `.brood_crash_dump` in the cwd, but a session crash does not.**
  If work vanishes with the session, the recoverable trail is: the scratchpad under
  `/tmp/claude-*/…/scratchpad/`, `~/.claude/history.jsonl`, and `journalctl -k | grep -i oom`
  (absence there means it was *not* the kernel OOM killer). Write findings to a file as you go.

**Measurement**

- **For a mechanism with an off-switch, the switch IS the attribution; a two-binary delta is only a
  hint.** One binary, one invocation, `MECHANISM=off` vs on. That disposed of a `nqueens` −5% in
  seconds (the switch is worth 0.3% there, so the mechanism cannot be worth 5%). Reach for it
  *before* building a fixed-baseline harness.
- **Hand-measuring against a `make ab` baseline requires `target/release-fast/brood`, not
  `target/release/brood`.** `make ab` builds both sides with `make release-brood` (profile
  `release-fast`); comparing its baseline against a `cargo build --release` binary compares two
  profiles and yields confident nonsense — it gave me `nqueens` −4.3% and `startup` −5.6%, both
  fictional. Footgun #1 in `ab-bench.sh`'s own header.
- **An optimisation whose mechanism is a fast-path test cannot clear the slow path, and a corpus
  that only exercises the fast path will report that it did.** The char-count cache was exactly
  that shape; so is anything gated on `is_ascii`. Sweep **both** encoding regimes.
- **A ratio near 4× that RISES across bases is not linear.** `format-source` read 3.80/4.12/4.64 and
  was cleared as linear; pushing the base gave 4.46 then 6.40. Only a *falling* ratio (warm-up)
  clears a row. Check the trend across triples, not one triple.
- **A row's "noise floor" can be an artefact of the metric.** `spawn-live` was credited with a
  20.6% floor and treated as unmeasurable for weeks; that was **wall** time on a 3.3-core
  workload. Measuring **CPU** time over a fixed unit count, with the two binaries interleaved,
  gives a <2% spread on the same row — enough to resolve a 12% change. Before accepting that a
  row cannot be measured, try measuring something else about it.
- **A cache that cannot be observed missing looks like a cache that works.** ADR-175's shared
  compiled-code cache shipped with the right mechanism and the wrong key; nothing in the suite
  or the benchmarks could tell, because the only symptom was slowness. The counter that catches
  it — compiles should be ~one per arm per *run*, never per *process* — is now `n_compile`, and
  `BROOD_TRACE_COMPILE=1` names the offender. Ask of any cache: what would I measure to see it
  missing?
- **Before believing a small two-binary delta, measure a row that CANNOT be affected by the
  change.** ADR-213's ASCII micro read `char-at` +2.1% and `last-index-of` +2.4% against 0.0%
  floors, and I spent two builds reshaping hot paths to chase it — the reshapes made it *worse*
  (+5.5%). `wordcount` calls none of the changed code and read **+2.2%** on the same binary pair:
  ~2% of whole-binary codegen/layout drift, and every string row was inside it. A size argument
  works too, and faster: `last-index-of`'s delta scaled with a byte scan costing ~90 µs per call,
  and no per-call change of a few instructions accounts for 2.4 µs.
- **Establish the noise floor first — and the floor measured *inside* one invocation does not bound
  the drift *across* invocations.** `nqueens` read −5.0% against a 0.2% base-vs-base floor while the
  same binary measured 104.6 and 107.6 ms in two best-of-15 runs.
- **A short row needs a mean, not a best-of.** `startup` is ~17 ms and `make ab` reports whole
  milliseconds, so it reads ±6% from quantisation alone; a 40-run mean gives +0.4%/0.4% floor.
- **Discard the first run after a fresh build.** Cold boot cache reports a ~44 MB base instead of
  ~24 MB in `process_floor.blsp` — the same size as the effect being measured.
- **Measure the slope, not the ratio.** RSS/N folds the runtime's ~24 MB base into a per-process
  figure; that is how the memory floor was once recorded as 5.9 KB when it is ~4.27 KB.
- **A harness can measure itself.** `process_floor.blsp` retaining the spawned pids put N cons cells
  of per-process cost into the very slope it measured (4470 vs 4271 B/proc).
- **Load contaminates everything.** `ring` "regressed" 12%, `supervisor` 8%, a sweep row 3.7% — all
  gone on a quiet machine. Wait for load < 0.5.
- **Never difference time-boxed runs** — RSS tracks iterations, so the comparison measures the
  iteration count.
- **A full disk looks like a toolchain crash** (`ld terminated with signal 7`). Check `df` first.
  `make ab-clean` is not automatic — ~1.1 GB per baseline worktree.
- **`RSS is not a proxy for live bytes`** here — but check before blaming the allocator:
  `MIMALLOC_PURGE_DELAY=0` moved the per-process floor by 2%.

**Testing**

- **"Cannot reproduce it locally" is evidence, not an obstacle — read what the passing configs rule
  out.** KI-27 passed 7/7 solo, 16/16 as concurrent copies of its own binary, and 3/3 as its whole
  binary under 12 CPU hogs, and failed only in a full `make test`. That set of results *is* the
  diagnosis: the cause has to be something only a full, heterogeneous suite supplies. It was other
  processes churning TCP connections, because the harness drew its ports from the kernel's
  **ephemeral range** (32768–60999) — so an unrelated client socket could be handed the port a
  test node was about to bind. Never allocate a test server's port with `bind(":0")`-and-drop.
- **A `Mutex` between tests does nothing under `cargo-nextest`.** The dist harness had a `PORTS`
  mutex around bind→spawn, which reads like the race was handled. nextest gives each test its own
  **process**, and `make test` uses nextest — so in the only configuration that fails, that
  mitigation does not exist. Any cross-test coordination has to be OS-level (the port band, a file
  lock), not a `static` in the test binary.
- **Copy-pasted harness helpers hide a bug in the copies you didn't look at.** `free_port` lived in
  three test files; fixing KI-27 in `distribution.rs` left the identical latent flake in
  `serve_attach.rs` and `observe_attach.rs`. They share `crates/cli/tests/support/mod.rs` now.
- **Build the concurrent reproducer before arguing about a flake rate, and compare failure *modes*
  not counts.** `live_migration deep_receive_…` failed inside a full `nextest` run and never in
  isolation (0/65). Two different failures were conflated: HEAD fails it on a *liveness* assert,
  while the change under test failed with an **out-of-bounds `root_at`** — a real GC bug. 16
  concurrent copies of that one test separated them in seconds: **8/16 vs 0/16 at HEAD**, where the
  full suite gave a 1-in-8 murmur that six baseline runs had failed to contradict.
- **A test-level `:isolated` inside a `describe` used to be silently dropped** —
  `register-test!` discarded the flag while collecting, so the marker did nothing and the suite
  reported `0 isolated`. Fixed 2026-08-05 (it rides in the meta; `emit-describe!` gives such a
  test its own isolated unit), but the lesson generalises: **a marker that is ignored rather than
  rejected is worse than an unsupported one**, because you believe the test is protected. Check
  the `(N isolated)` count in the summary when you rely on it.
- **A green test proves nothing until you run it with the mechanism off.** For any mechanism with an
  off-switch, run the test with the switch off before committing it. For a mechanism with **no**
  switch (a pure cache, like ADR-213's index), the equivalent is **sabotage**: break it by one
  character and confirm every new test fails. If they don't, they are not gates.
- **Verify a detector before trusting it — but "make it fire" need not mean "reproduce it end to
  end".** KI-26's runtime detector could only fire by winning a race, and never did across the
  suite, `pfib`, and a 24-process purpose-built race. The hazard was a *predicate*, so extracting
  the predicate and table-testing it was both possible and stronger: it covers both flag states and
  every nearby frame size, which no amount of hammering would.
- **Before adding a nextest retry, ask what else that test guards.** `live_migration`'s liveness
  assert flakes under load, but its *other* assertion catches intermittent continuation corruption —
  the very bug it caught in ADR-210. A retry would have absorbed that as FLAKY. Fixed by raising the
  burst budget instead (8/60 → 0/60, free on a normal run).
- **A derivation firing is not an optimisation landing.** `BROOD_INLINE_DBG` reported a
  partially-spliced derivation for `row-sum`; `BROOD_JIT_DUMP_IR` showed it never lowered. Leaf
  inlining is JIT-only and the VM always runs the small body, so a bailed arm gets nothing. A bailed
  arm never reaches the `[jit-ir]` dump, so *absence* there is the signal.
- **`std/*.blsp` is embedded at build time.** Rebuild `brood` **and** `nest` after touching `std/`,
  or you will debug yesterday's bytes. Same class as `-p brood` vs `--bin brood`.
- **The conformance tests need `nest test`, not `--test`** — they `(:use corpus)`, which only
  resolves through the project's module path. `brood --test tests/conformance_utf8_test.blsp` fails
  with "cannot find module 'corpus'", which is a harness error, not a failure.
- **`pkill -f <pattern>` matches your own shell** — it killed my own command twice in one session.
  Use `pgrep -f "[h]arness.py"` and kill by PID.
- **Process death reports go to stdout** — `2>/dev/null` will not filter them.

**Diagnosis**

- **"Restrict the scope" is not automatically simpler than "synchronise".** I had written up
  LOCAL-slots-only as the smallest sound way to populate ADR-213's index, because the shared regions
  race; a `OnceLock` turned out to be *smaller* (no region split in the accessor) and broader. When
  the cached value is a pure function of immutable data, a race between builders is benign — and
  this kernel has immutability everywhere (ADR-026), so reach for that argument before narrowing a
  feature's reach.
- **When a fix underdelivers against a mechanism you were confident about, that gap is evidence
  about where the cost actually is.** The `sexp` allocation fix I predicted was worth −18%; being
  disappointed by it sent me back and found the real one (two O(point) passes where one suffices,
  −39% more). I would otherwise have written up a true and incomplete story.
- **Threads get named after the mechanism nearest the symptom, not the cause.** Two of four were
  misnamed — and §1 was misnamed *again* on 2026-08-05: "cold inline caches" was the nearest
  plausible mechanism to "a fresh process is slow", so it became the item, and a ladder delta was
  read as confirming it. One measurement (a per-call warm-up curve) disproved it. Re-derive a
  thread's premise before implementing against it, and prefer a curve to a counter.
- **A deopt's `resume_ip` names the nearest CHECKPOINT, not the failing guard.** Checkpoints sit
  after calls, so a call-mediated arm's deopt always *looks* like it happened at the call — and
  removing the call moves the reported site to the next call, which reads as confirmation. It
  is not: with every call removed the arm still deopted, now with no checkpoint at all. To find
  a guard, read the CLIF (`BROOD_JIT_DUMP_IR`) for the branch into the deopt block; the ip is a
  hint about *where execution resumed*, never about what failed.
- **An off-switch measuring flat can mean the path was never taken.** `BROOD_NO_HOF_JIT=1` moved
  the receive micro 0.0% — which reads as "the native fast frame is worthless here" and is
  wrong. `jit_link_done = 0` showed the frame had never engaged on a single call. Before
  concluding a mechanism is worth nothing, check that it *ran*: for a mechanism with a success
  counter, the counter beats the A/B.
- **A probe that doesn't exercise the path reports confidently about nothing.** An A/B of two
  closure shapes through `fold` was built to test the HOF-step deopt — but `fold` is *Brood*
  (`fold--vec`), not a Rust HOF driver, so `hof_resolve` was never called and both arms'
  tallies were other arms' work. The tell was there: the `resolve:OK`/`step:enter` counters read
  **zero**. Check that your instrument fires before reading its output.
- **A counter is not a timing.** IC misses ran at 58% of call sites, which was true and cost ~2 µs
  of a 33 µs row. A high miss *rate* is unavoidable on a process that makes five calls; only the
  `ns_*` timers and a per-call curve sized it.
- **A comment asserting a cost is not evidence** — and a docstring can describe a bound's *intent*
  rather than its achievement. `sexp/narrow` says motions cost "~three forms, not the whole buffer";
  true of the CST work it wraps, false of finding the window, which was the whole cost.
- **Read the existing argument before inventing one.** ADR-194's comment named exactly why sharing is
  sound on the parked path, which identified what the serialised path lacked.
- **A benchmark port drifts silently when language semantics change under it.** `mandelbrot` looked
  like a 3.5× regression bisected to exact rationals; identical source measures 201 vs 200 ms —
  `(/ px n)` had simply stopped being a float divide. When a numeric primitive's semantics change,
  grep the benchmark ports.

## 7. Semantics worth knowing (documented, not bugs)

- **Char indexing costs O(1) in both encoding regimes** (ADR-213). A char index *is* a byte offset
  for pure-ASCII text; off that path the string slot's sparse char→byte index makes the conversion a
  lookup plus a walk bounded by 32 chars. So `substring` is O(result), and a `char-at` loop or an
  `index-of` scan with a rising `from` is linear on any text. The code-point-vector rewrites this
  class of bug once forced (`url`, `csv`, `ansi`) are no longer required — they are left alone.
- **A cache keyed to a string VALUE goes on the slot's `StrAux` cell, not in a map keyed by
  `StrId`.** A handle is unique only within a GC epoch; the cell travels with the bytes. The heap
  owns the cell as `dyn Any` and never interprets it, so a higher layer can cache its own table
  (ADR-214's lexer safepoints) without the core depending on it.
- **`(buffer-text buf)` is `rope->string` — a fresh O(n) string per call.** Anything that calls it
  per keystroke is O(buffer) per keystroke before it does any work of its own. This is why the
  editor gets nothing from ADR-214 (§2).
- **Hot reload reaches a top-level `defn` self-loop, but not an inner `letrec` loop.** A tail
  self-call compiles to `Node::SelfCall`; since 2026-07-30 (commit `4bbef7d9`, guarded by
  `tests/vm_selfcall_reload_test.blsp`) it watches the global epoch and re-resolves its own global
  name on a `def`, so a running `(defn serve (s) … (serve …))` *does* adopt its own redefinition on
  the next back-edge. But when the back-edge targets a **local gensym** (a `letrec` loop — the shape
  `defserver` expands to) there is no global to re-resolve, so it keeps old code; only globals the
  body calls by name late-bind. Erlang's local-vs-remote rule; see `live-editing.md` (and Stage 6 /
  the ROADMAP `code_change` item for the state-migration hand-off).
- **A closure that captures no locals is already shared code**; one that captures a local is copied
  on send. That is why supervisor `:start` thunks should avoid captures — ADR-194/208.
- **`/` is exact.** `(/ 3 4)` is the rational `3/4` (ADR-196); `(/ 4 2)` is `2`. Use `quot` for an
  integer count, and convert to float *before* dividing in a float pipeline.
- **`->float` is a function call, not a cast** (~85 ns).
- **Leaf inlining is JIT-only** — the VM always runs the small body, so an arm whose native bails
  gets nothing from any splice.
- **Duplicate supervisor `:id`s** resolve to the later-started child.

## 8. Tools

Startup / project-scale measurement lives in **`scripts/bench/`** (added 2026-08-07, because the
last session's ladder was left uncommitted and its figures could not be re-derived):

- **`gen-project.py N DIR`** — a synthetic project of N modules × ~180 lines, with an entry point
  that reaches exactly TWO of them (the case the lazy image exists for). The 10x-moneyclub shape
  is `16300`.
- **`image-scale.sh [sizes…]`** — per N: cold load alone, cold load + image write, image size, so
  the write is attributable. Waits for an idle box, discards a warm-up run, and its header lists
  the four traps that each produced a wrong number here first (load contamination, cold boot
  cache, `nest` vs `brood` build-ids, and `nest run` not being the loader).

Flake-hunt tooling for the boot-cache family lives in **`scripts/ki38/`** (committed for the same
reason as `scripts/bench/` — the previous session's equivalent sat in a scratchpad and its figures
could not be re-derived). All of it reads `/proc` only and spawns no `brood`, so it cannot
re-create the KI-29 orphan leak an earlier sampler did:

- **`bootcost.sh [N]`** — what a cold expanded-prelude boot costs, alone and as a herd of N,
  against the warm path, isolated via `XDG_CACHE_HOME` so the real `~/.cache/brood` is untouched.
  This is the tool that separates the two boot paths; use it before quoting any boot number.
- **`doseresponse.py`** — sweeps herd size against a shared cold cache and reports the worst boot
  per herd: the number the 20 s / 30 s deadlines actually race. Aborts the sweep on memory
  pressure rather than risking the box.
- **`sysmon.py OUT.csv`** / **`analyze.py OUT.csv`** — a 1 Hz timeline beside a suite run and its
  summary (loadavg, MemAvailable/SwapFree/Shmem, major-fault and swap deltas, live `brood` count
  and how many are in `D`, and which test binary is running so the timeline names the schedule
  region). `analyze.py` is worth running on **green** rounds too: that is how the memory
  hypothesis for KI-38 was refuted, before any sighting.

Everything else is in `scripts/fuzz/stress/`, each with a usage header worth reading first.

- **`scale_sweep.blsp`** — a `std/` op at N and 4N, ratio printed (linear ~4×, quadratic ~16×).
  **`UTF8=1` re-runs every row in the multi-byte regime.** Its header records which rows are
  cleared, which were cleared *wrongly*, and why. **Every row is linear today**, so a
  superlinear reading is a regression, not a known gap.
- **`leaf_splice.blsp`** — partial leaf splicing's benchmark (ADR-210); ~220 ms vs ~520 ms with
  `BROOD_NO_PARTIAL_LEAF=1`. Its header carries the derivation-vs-lowering trap.
- **`spawn_live_ladder.blsp`** — decomposes the worst published row into rungs
  (`spawn`/`send`/`nopark`/`park`/`park-batched`/`payload`). **One rung per process** (a shared
  process leaks JIT tiering into later rungs and breaks monotonicity — which is the tell) and
  **CPU, not wall**. `park-batched` with `BATCH` separates suspending from coexisting, the
  confound that made item 2 look like a 5.5 µs lever. Verify any rung's parking claim with
  `BROOD_L1_STATS=1`: the fast path fires only on a parked receiver, so it counts parks
  directly.
- **`recv_matcher.blsp`** — what one `receive` costs, in a single long-lived process (so
  tiering is not a variable, unlike the ladder). Four modes, and the pairing is the point:
  `PAT=vec` (`[:go v]`) and `PAT=intlit` (`[1 v]`) compare a literal element and never reach
  the native fast frame; `PAT=bind` (`[a v]`) and `PAT=any` (`m`) do not compare one and stay
  native. **`bind` is the control to quote** — same vector work as `vec`, differing only in
  the compare, so the −28% between them is a real gap rather than work not done.
  `jit_link_done` — 0 vs ~N — is the counter that says which side of the fast frame you are
  on; no other counter shows it (`jit_native` stays high because the *caller* is native).
- **`hof_call.blsp`** — per-call cost of a HOF step function, global vs computed head. Its
  header carries the trap that makes this measurement easy to get 160× wrong: a callee of the
  form `(defn f (a b) (%prim a b))` is a passthrough that `resolve_prim` **inlines**, so such
  a row measures a deleted call, not a call. Prints total ms and the accumulator beside every
  figure so a vanished loop is visible.
- **`process_floor.blsp`** — the per-process idle floor; ~4.27 KB, flat across N. Read the slope,
  never `rss/n`; discard the first run after a fresh build.
- **`soak_selfcheck.blsp`** — sustained load with an invariant checked every iteration. **Always pair
  it with a control** reverting the mechanism under test.
- **`decay_isolate.blsp`** — throughput per fixed-size window plus RSS and `:runtime-closures`; run
  modes sequentially.
- **`receive_backlog.blsp`** · **`net_framed_scale.blsp`** — the receive-mark and framed-read
  benchmarks, each carrying its own controls.
- **`tests/registry_test.blsp`** / **`tests/shared_closure_msg_test.blsp`** — each carries a control
  that fails with its mechanism off, which is the only version worth having.
- **`tests/collection_identities_test.blsp`** — seeded-random laws over maps/vectors/strings,
  including the multi-byte char-index laws (each op against the code-point vector). The place to add
  a property that every engine would agree on, which the engine-differential is therefore blind to.
- **`BROOD_PERF_STATS=1` on a `--features perf-stats` build** — counts *and* (new) `ns_*`
  timing shares: spawn / deliver / message copy each way / receive / matcher resolve /
  teardown / one scheduler quantum (which nests the rest). This is what attributes a
  *process-shaped* cost; the counters alone could not. Pair with `BROOD_TRACE_COMPILE=1`.
- `scripts/fuzz/run.sh <generator>` — differential across 4 engine configs (tree-walker, VM-no-JIT,
  VM+JIT, GC-stress+verify). `make ab BASE=<ref>` for brood-vs-brood rows; `bench/harness.py` in
  brood-benchmarks for the published cross-language numbers.

## 9. Where we stand against the field

From the published run (`brood-benchmarks/results/`, **2026-08-10**, brood 0.3.9 at commit
`853afe6f` — clean exit, no checksum mismatch, no compute-floor clamp):

- **`spawn-live`** — still the worst row, and it has now moved **four runs running**:
  1.93 → **1.75 s** (−8%), 5.55 → **4.66 CPU·s** (−16%), RSS 1.57 → **1.60 GB**. Now **2.5×
  slower and 1.8× heavier** than the BEAM (~5.5 KB per live process against ~3.1 KB), from
  2.8×/1.75×. This run's move is `%vector-reduce` plus `fold`'s vector-first dispatch, both
  A/B'd against a fixed baseline *before* the harness (9.96 → 8.04 then 8.30 → 7.92 CPU·s,
  −20.5% together, against −16% observed). The RSS rise was predicted by the same A/B. §1
  has what is next; note the receive-machinery lever is now **retired**, measured at ~1.8%.
- **`latency`** (open-loop, ranked by p99) — Elixir 58 µs, **Brood 73 µs**, Python 467, Node
  472, .NET 839. 2nd of five, p50 16 µs against Elixir's 8, p99.9 492 µs. Essentially flat
  against the previous run's 75/488 on an unchanged message path. The level is what earlier
  runs bought, and the reason is worth keeping: p99.9 658 → 461 and max 6.0 → 1.6 ms came from
  ADR-215 removing compilation from the *arrival* path. **An open-loop tail is where one-off
  per-process setup shows up**; a throughput row amortises it away.
- **`supervisor`** — Brood 847 ms vs Elixir 254 ms, unchanged in substance.
- **Overall speed** — **2.3× the leader (4th of seven)** over all 27 rows every port
  implements, geomean of per-row ratios, normalised so the leader reads 1×. This replaced an
  aggregate over 11 rows summed by wall time on 2026-08-10; the wider set is *not* harsher on
  Brood in absolute terms (2.3× against 2.8× on the eleven compute rows), but it drops Brood
  3rd → 4th because Elixir gains more when the concurrency rows count. The narrower
  single-threaded compute aggregate is unchanged at **2.8× the fastest**.
- **Base RSS 23.2 MB** — 3rd-lightest of seven. **Chased 2026-08-10 and there is no regression
  to chase.** Three lean builds, identical flags, warm: `4f49a38f` (pre-brotli) 21.5 MB,
  `877ccec5` (brotli) 21.3, `853afe6f` 21.8 — **+0.3 MB across the window**. Brotli added
  **1.2 MB of binary and zero RSS**, because code that never runs at boot is never paged in.
  And the metric is **bimodal**: a cold expanded-prelude boot costs **~42 MB** against ~22 MB
  warm, an 18.9 MB swing that is what the published history's 12.8–27.9 MB spread actually
  tracks. Treat a sub-2 MB movement as drift unless a controlled build says otherwise. The
  older 18.6 → 22.4 step predates this and stays attributed to the crypto deps + ADR-215.
- **A per-row trend chart exists now** (`bench/trend.py` → `results/trend.svg`): Brood's rows
  across every published run, from git history of `results.json`. `spawn-live` is **−67%**
  across seven runs (5362 → 1771 ms) — progress that is invisible on the positioning map,
  because one row inside a 27-row geomean barely shifts it. Use it to see whether an
  optimisation landed; use the map for where the runtime stands.

**Publishing procedure** (from `brood-benchmarks/CLAUDE.md`, and it matters): install the **lean**
build first — `make install INSTALL_FEATURES='$(RUN_FEATURES)'` — run `python3 bench/harness.py`
at its defaults on a quiet machine with no concurrent builds, then update by hand in this order:
`bench/chart.py`, `BENCHMARKS.md`, `README.md`, `FRONTIER.md` (only if a gap materially moved).
The harness fails itself on a checksum mismatch or a compute-floor clamp, so a clean exit means
something. One trap of its own: `pgrep -f "bench/harness.py"` is useless as a wait condition —
stale waiter loops from earlier sessions match that pattern (and match themselves). Wait on the
PID.

## 10. `nest run`'s cold pre-flight — fixed, and what is left in it

`check-project-run-closure` reads its closure off `*features*`. On a **warm** start that is the
handful of modules the entry materialised, and it is cheap. On a **cold** start every module has
just been loaded to build the image, so the closure is the whole project — and each file was
handed that whole list as its KI-17 reachability set. The check fans across the green-process
pool, and **a `spawn` deep-copies what its body captures**, so the list crossed the heap boundary
once per *file* (~100× per chunk): copies = files × closure, quadratic in project size.

Fixed 2026-08-07 by `project-pfold-files-shared` + the `:check-shared` op, which prepend the
shared set to each chunk instead of pairing it with each file. Same files, same warnings —
verified by running the old per-file path and the new one in the same session against a project
with three deliberate warnings and diffing the output (identical). Measured, interleaved, idle:

| N | before | after |
|---|---|---|
| 4 000 | 16.1 s / 2.6 GB | 14.0 s / 1.3 GB |
| 8 000 | 36.6 s / 8.4 GB | 28.9 s / 2.1 GB |
| 16 302 (cold `nest run`) | 81.6–83.5 s / 26.76 GB | 84.1–88.0 s / 5.2 GB |

Per-doubling memory growth 3.2× → 1.6×. The ~+4% time cost is real (the samples do not overlap),
and worth it at the size where it matters.

**What remains, if this row is ever revisited:** the checking itself is ~3.7 ms/file and verdicts
are deliberately not cached, so a cold `nest run` on a 16 000-file project still spends ~50 s
checking code the entry point may never reach. The honest fix is to scope the pre-flight to the
entry's *require closure* computed from module headers rather than to `*features*` — which is
what the docstring already claims it does. That is a behaviour change (fewer files checked ⇒
fewer advisory warnings on a cold run), so it wants a deliberate decision rather than a
drive-by; it was not taken here for that reason.

**A trap from that measurement.** A single post-fix `nest run` read 139.6 s, which looked like a
61% regression and contradicted the 4 000/8 000 rows (both got faster). It was the box state —
that run followed a full suite. Two lone samples taken hours apart are not an A/B, even when both
look clean. The interleaved four-sample run settled it at +4%.
