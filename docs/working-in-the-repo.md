# Working in the Brood repo — reference

`CLAUDE.md` holds the rules and invariants that apply to every change. This page holds the
reference material it points to: what each `nest` subcommand is, the build and CI commands
in detail, the debug tooling, and the checklist for a stdlib rename. The module map is
[source-map.md](source-map.md), generated from the code.

## The two binaries

The CLI is split (ADR-028): **`brood` runs the language**, **`nest` runs the project**.
Both embed the `brood` lib. `nest` subcommands: `new`, `test`, `check` (`--fix-renames`
applies the unambiguous half of a rename wave from the ledger, ADR-304), `run` (refuses to
launch over an unbound symbol in the entry point's require-closure; `--no-check` skips it;
`--watch`; `--check-boot` loads every module and resolves `:main` but runs nothing, KI-66),
`doc`, `docs`, `doctest`, `format`, `repl`, `mcp` (an MCP server over the project, ADR-036),
`observe`, `attach` (thin frontend for a `ui-run` daemon, ADR-090), `completions`/`complete`,
`grammar` (editor grammars from `(special-forms)`, ADR-092), `rename`, `ws`, `stdimage`, the
package manager (`fetch`/`update`/`tree`/`add`/`remove`, ADR-037; `publish`/`search` against
the hosted `hive` registry, ADR-147/211; `key`, ADR-212), `release` (bundling, ADR-038;
boot-checks the binary it wrote and deletes it on failure; `--no-smoke` opts out), and
`update-tooling`/`update-brood`.

## Commands in detail

- **After a push, `make ci-wait` — never a hand-written `gh run list -L 1` loop**: right after a
  push that returns the PREVIOUS run, and every push cancels the run before it; `ci-wait`
  follows the newest run whose commit contains yours and prints the failing tests.
- **Waiting for a build: never `while pgrep -f "cargo build"`** — the pattern matches the waiting
  shell's own command line, so the loop never ends. Wait on the build command itself.
- **`make green` answers "is the tree green?" — never hand-read the run list.** A cancelled
  CI run is not evidence (every run is cancelled by the next push), and `make check` is not
  what CI runs: CI also runs `nest format --check`, the zero-warning checker gates, the
  examples and stress gates, and a `downstream-bedit` job (bedit at `BEDIT_REF`;
  `make smoke-bedit` is the same locally). `make green` prints **"no verdict"** rather than
  "green" when CI has concluded nothing recent (KI-68/KI-69).
- **Clippy: `--all-features` is load-bearing.** CI runs
  `cargo clippy --all-targets --all-features -- -D warnings`; a plain
  `cargo clippy --all-targets` passes code CI rejects. A failing clippy step skips every
  CI step behind it. Every `unsafe` block and impl needs a `// SAFETY:` comment
  (`undocumented_unsafe_blocks`, workspace lint).
- **`make test` runs via `cargo-nextest`**: each test in its own process (a SIGSEGV is
  contained to one case), hard-capped per case (`.config/nextest.toml`). `cargo test` has no
  per-test timeout and can hang. `make ensure-nextest` installs it.
- **`make check-desktop`** runs CI's static gates (fmt, `nest format --check`, `nest check`
  and `--strict` as CI spells them, clippy) plus `TEST=tests/x_test.blsp` if given — the
  recipe before a `--no-verify` push from a machine that runs no suites.
  **`make affected-tests`** prints the tests that name what the current change touches.
- **Perf-benchmark builds: `--bin brood` (or `-p cli`), never `-p brood`.** `-p brood` builds
  only the lib and does not relink the binary, so an A/B silently compares stale binaries.
  The JIT is a default feature (ADR-101). The tier ceiling is **`BROOD_TIER=0|1|2`**
  (tree-walker / bytecode VM / native, ADR-222) — the only engine knob.
- **Measuring performance:** `docs/benchmarking.md` §6 has the rules and why: `make ab
  ARGS=--floor`; `make ab-vm` for the VM call path; a base-vs-base control before believing a
  few-percent delta; re-run a tiering-sensitive row unpinned; measure a short and a long run;
  **count the calls** before optimising anything; attribute with `make perf-brood` +
  `(perf/summary)` (its own profile, `target/release-perf/`, never the timing binary). Run
  `make doctor` first — a stale binary fails by agreeing with the baseline.

## Debug tooling

**`brood --debug-flags` lists every `BROOD_*` flag the runtime reads**, grouped for triage,
from `crates/lisp/src/diagnostics/debug_flags.rs`. Both directions are gated by tests: every
catalogued name must exist in the source, and every `BROOD_*` read under `crates/*/src` or
`std/` must be catalogued. That file is the reference — add a flag's description there, not
here. The ones you reach for first: `BROOD_GC_STRESS` / `BROOD_GC_VERIFY` (GC races),
`BROOD_TIER` (which engine), `BROOD_J=1` (one worker), `BROOD_JIT_DUMP_IR` /
`BROOD_JIT_BAIL_TRACE` (did an arm lower, and why not), `BROOD_PERF_STATS` (counters; needs
`make perf-brood`).

**Build with `RUSTFLAGS="-C debug-assertions=on" cargo build --release`** to keep release
speed while arming every debug check (plain `--release` strips them; plain debug is correct
but too slow to expose contention races).

**Two layers of use-after-GC detection** (a moving collector relocates LOCAL handles; a
handle held across a collection without re-rooting goes stale):

1. **Per-deref tripwire** (always on under debug-assertions). Every LOCAL handle accessor in
   `core/heap.rs` checks a poison bit + a generation epoch (`check_epoch_aged`), panicking at the
   *instant of the bad deref*. Catches a stale handle that's **dereferenced**.
2. **Heap verifier** (`BROOD_GC_VERIFY=1`, `Heap::verify_local_graph`, works in plain
   release). The tripwire misses a stale handle **stored** into a heap cell without being
   deref'd (it surfaces far away — an OOB slab index, or `promote` recursing a corrupted
   graph to a `SIGSEGV`). Before each collection the verifier walks the live LOCAL graph and
   panics naming the stale handle and **the cell that holds it** (`0` = a root) — its
   immediate parent, not a full root path — at the store site's next collection, so you find
   the missed-rooting site, not the distant blow-up. Reach for it when GC_STRESS gives a
   `SIGSEGV` or a raw index panic rather than a clean tripwire message.

**Crash artifacts.** `brood`/`nest` install a panic hook (`cli_support::install_crash_dump`)
that appends the panic + backtrace to **`.brood_crash_dump`** in the cwd — durable when a TUI
scrolls the message away; at 16 MiB it rotates to `.brood_crash_dump.old`, replacing the last
one. It catches Rust *panics*, **not** `SIGSEGV`: a fatal signal (SIGSEGV, SIGBUS, an abort)
leaves a one-line record under `$XDG_STATE_HOME/brood/crashes/` instead (ADR-635,
`crash-report/previous-crashes`), with no backtrace; for the stack use
`gdb --batch -ex run -ex bt <test-binary>` (`rr` isn't installed, and `valgrind` won't see a
*logical* use-after-GC over safe `Vec` slabs).

## When you rename or reorder a stdlib function

A rename wave or argument reorder is not a find-and-replace (ADR-302/308 cost ten bugs, six
silent). **Enumerate these before the first edit.**

**A call has five spellings**, and a textual rewriter sees only the first: bare `(map xs f)`;
qualified `(string/join xs sep)`; root-scoped `(/map xs f)`; macro-constructed
`(list '/mapv …)` (ungreppable until expansion); and string-embedded — `%load-string`,
`reflect/eval-string`, and `nest new`'s scaffold templates (a sweep finds ~118 strings with
`(map …)`, ~15 executable — read them).

**Code generators are outside every static gate** — the fuzz generators
(`scripts/fuzz/generators/*.blsp`, `stress/fuzz_programs.blsp`) write Brood as strings, so
grep them too; `crates/cli/tests/fuzz_generators_live.rs` runs each generator once and fails
on an unbound symbol. The scaffold templates are covered by
`crates/nest/tests/scaffold_quality.rs` (format, check, tests, `--check-boot`, and a full run
for the templates whose `main` terminates).

**The Rust checker encodes argument positions structurally**, and they degrade to `any`
*silently* — `types/check/sigs.rs`, `infer.rs`, `walk.rs`. Two gates must stay green:
`types::check::sigs::convention_tests::curated_combinators_put_the_callback_last`, and
`tests/introspection_test.blsp` "combinator result inference survives the argument order"
(pin a type the checker can actually prove; `any` is what a broken position yields too).

**`map` is a function name AND a type name.** `(map K V)` in a `sig` is a type; `(map -> int)`
parses as a two-argument call. Check `(sig …)` **and** `(sig! …)`.

**Prefer the reorder that fails loudly.** Moving a collection into argument one fails at once;
reordering a *callback's own* parameters fails silently (ADR-308 keeps `(fn (acc x))`).

**`doctest` is the completeness gate.** `tests/doc_examples_test.blsp` and
`tests/doctest_test.blsp` execute every `form → result` docstring example. Run them early.

**A rename wave ends downstream.** Put every moved public name in the rename ledger
(`crates/lisp/src/renames.rs`) so bedit's `nest check` says `renamed to …` and
`--fix-renames` applies it; fix and push bedit; then `make smoke-bedit ARGS=--bump` (moves
`BEDIT_REF` to bedit's pushed HEAD) and commit the pin with the change. Smoke with a
grammar-enabled `nest` (`brood/treesit-grammars`), or bedit's mode tests fail on
"no grammar for :elixir", which is not a regression.
