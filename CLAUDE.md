# CLAUDE.md — working in the Brood repo

Guidance for Claude Code (and humans) working in this project. For the broader
machine setup (Ubuntu, apt, Rust via rustup, etc.) see the global
`~/.claude/CLAUDE.md`.

## What this project is

Brood is a dynamic **Lisp implemented in Rust** with a deliberately small core.
It began as the language a modern, Emacs-like, self-editing, remotely-hostable
editor would be written in, and has grown into a general-purpose language and
runtime; this repo is all of Brood — the language core, the runtime (processes,
distribution, VM + JIT), the standard library, and the `std/editor/*` framework
for interactive applications. Read `docs/` before making non-trivial changes —
especially `docs/architecture.md`, `ROADMAP.md`, and `docs/decisions.md`.

Brood source files carry the **`.blsp`** extension (*Brood Lisp*; `.lisp` collides with
Emacs' `lisp-mode`). Any `.blsp` file, or a reference to "blsp", means **Brood-language
source**, as distinct from the Rust kernel under `crates/`.

**When writing Brood code, load the `writing-brood` skill first**
(`docs/writing-brood-skill.md`, ~160 lines): the traps and idioms that differ from other
Lisps. `docs/brood-for-claude.md` is the full reference behind it. `nest new` drops both into
every scaffolded project (they are baked into the binary via `%builtin-doc`).

## Greenfield: prefer the right structure over compatibility

This is **greenfield** — pre-1.0, no external users, nothing to keep stable.
**Make breaking changes freely when they improve the overall structure.** Don't
add compatibility shims, deprecation aliases, or "keep the old way working too"
hedges; rename, re-shape, or delete the old thing and update every caller. Keep the
build/tests green, and record notable breaks in `docs/devlog.md`.

## Green tree first — don't start new work over known flakiness or bugs

**Before picking up any new feature or perf work, the tree must be provably green.**
A known open bug, a flaky test, or an unexplained failure — *even one seen "only
once"* — **is** the work: fix it, or prove it a non-issue, before building on top. A
flake left running erodes trust in every green run after it.

> **Where tests run: never on the development desktop.** Every test, suite, stress, fuzz and
> benchmark run below happens on the test machine or in CI — not "light", not "targeted",
> not niced. On the desktop: build, format, push when asked, and read CI (`make green`,
> `gh run`); say plainly which tests were not run. The run rules that follow (what counts as
> green, the address-space cap) are for whoever runs them on the test machine.

- **Confirm no open issue.** `docs/known-issues.md` should show no open bug. A *watch*
  item is acceptable only if it genuinely cannot be reproduced on demand — say so, with
  the diagnostic armed.
- **Prove green, not "passed once."** For anything touching concurrency, the scheduler,
  dist/nodes, GC, or the JIT, the relevant tests must pass *repeatedly* on the test machine
  or in CI — `make test-both`, the distribution tests looped, `BROOD_GC_STRESS=1
  BROOD_GC_VERIFY=1` (`make gcstress`), and a fuzz-differential pass (`scripts/fuzz/run.sh`).
  See `docs/handoff.md` § Flake hunting.
- **Gate performance at the tier your change runs on** — `make ab-vm` as well as `make ab`
  for anything on the VM's call path (`docs/benchmarking.md` §6).
- **A regression is stop-the-world.** If new work turns a green row red or a green test
  flaky, fixing that takes priority over finishing the feature.
- **Build uncapped, run capped.** Put an address-space cap in front of every test,
  `nest check`, `nest run` (downstream projects too) and `brood` run —
  `( ulimit -v 16000000; cargo nextest run -p brood -j1 -E '…' )` — and keep `-j1`, so one
  runaway is the whole spike. **16 GB, not less:** the runtime *reserves* ~3 GB of address
  space before doing any work (allocator arenas + worker stacks), so a 4 GB cap fails the
  first test that creates a `table` with a Brood error naming the cap. A *diverging* process
  is indistinguishable from a heavy one until it has eaten the machine (KI-87: 19 GB test
  processes, a 54 GB `nest run`); under the cap it dies in seconds with the panic site named.
  **Never put the cap in front of a build** (`cargo build`, `nextest --no-run`) — the linker
  inherits it and dies with `LLVM ERROR: out of memory`. The Makefile's test targets apply
  the cap to the run step only (`RUN_CAP`). A capped run that reds a wasm file is a real
  failure: the wasm host reserves only what `GuestBudget` allows (`host/wasm.rs`), so there
  is no "it's the cap" exception.

## Core principle: write the language in the language

**As much of the system as possible must be written in Brood itself, not in
Rust.** This is the most important rule in this repo — it is the entire reason
the project exists (a self-editing editor is only possible if its behaviour
lives in code you can redefine at runtime).

- Rust provides **mechanism**; Brood provides **policy**. Use Rust only for what
  genuinely needs it: primitives the language can't bootstrap (low-level I/O, the
  rope/text engine, performance-critical kernels) and the core evaluator itself.
- Everything else belongs in `std/` (Brood source), not in a builtin. When you reach
  for a new Rust builtin, first ask: *can this be written in Brood on top of existing
  primitives?* If yes, do that instead.
- The editor commands, keymaps, UI, the REPL (`std/tool/repl.blsp`, ADR-048) and the
  `nest` CLI dispatch (`std/tool/nest.blsp`, ADR-322) are Brood; the binaries bootstrap
  into them. `main.rs` routes a name in its `BLSP_SUBCOMMANDS` to Brood before clap runs.
  `nest mcp` is Brood too (`std/tool/mcp.blsp`); `crates/nest/src/mcp.rs` only supplies
  the documents the binary embeds. Still Rust: `stdimage`, which must build from a process
  where only the prelude is loaded (KI-112). `release` is split: what a release binary is called, which names are
  refused and what the command prints are Brood (`project-release`); the Rust side keeps
  the embedded runtime, the byte assembly and the boot check.
- A Rust builtin is an admission that the language can't yet express something.
  Treat each one as a candidate to later replace with Brood.

### Dogfood first; optimize only by building the language up, not around it

**Keep code in Brood even when it's slower, so we surface the language's real
gaps.** Reaching for Rust to make a slow Brood function fast *hides* the gap
instead of fixing it. Before optimizing, the change must satisfy **both**:

1. **It improves overall language performance** — a capability that pays off broadly,
   not a one-off speed-up for a single call site.
2. **It builds up the right primitive/capability** — it makes the *language* more
   capable, rather than moving behaviour out of Brood into a Rust escape hatch.

Worked example: variadic `+`/`-`/`=` (Brood `defn`s over `fold`) cost ~40× a direct
call. The wrong fix is making them Rust builtins. The right fix is giving the
*evaluator* efficient **multi-arity dispatch** — a general capability that keeps `+`
in Brood and makes *every* multi-arity function faster.

## Layout

**[`docs/source-map.md`](docs/source-map.md) lists every kernel and std module with its
purpose.** It is generated from each module's own doc line and a test fails when it is stale,
so it is current; read it to find where something lives. In brief:

```
crates/lisp/src/   the language and runtime; the directory tree mirrors the layers (lib.rs):
                   core (values, heap, GC) · syntax · eval (tree-walker, VM, JIT lowering,
                   macros) · types (the checker) · builtins (one file per domain, each owning
                   its implementations AND its `register`) · boot · process · dist · host
                   (feature-gated machine bindings) · diagnostics · jit (ABI; rt.rs is the
                   ONLY heap/GC interface native code has)
                   A parent is `foo.rs` beside its `foo/` (never `foo/mod.rs`).
crates/cli/src/main.rs   the `brood` binary — the language (REPL, file runner, `--test`)
crates/nest/src/         the `nest` binary — a thin shell over std/tool/nest.blsp — ADR-028
crates/lsp/src/main.rs   the `brood-lsp` language server (ADR-025, docs/lsp.md)
crates/playground/src/   the wasm-bindgen in-browser playground
crates/tab/src/          `brood-tab`: the wasm-bindgen shim that runs a bundled program in a browser tab
std/                     the standard library in Brood (ADR-085): prelude/ (13 bare-root
                         files concatenated in the order boot.rs lists) + bare-core modules;
                         `proc/*` (gen, supervisor, agent, crash-report), `net/*` (http, sse,
                         tcp, reconnect), `tool/*` (the toolchain: test, project*, nest, repl,
                         mcp, …) — grouped on disk but with BARE module names (`gen/spawn-server`,
                         `http/get`, `test/run` — never a double slash). `editor/*` is the one
                         exception and keeps its prefix. `bit`, `decimal` and `proc` also
                         namespace KERNEL primitives (ADR-251).
docs/                    architecture, language, roadmap, decisions, devlog, known-issues,
                         handoff.md (current state — read first when resuming work)
```

The CLI is split (ADR-028): **`brood` runs the language**, **`nest` runs the project**.
Every `nest` subcommand is described in
[`docs/working-in-the-repo.md`](docs/working-in-the-repo.md), which also holds the command
details, the debug tooling and the rename checklist this file points to.

## Commands

**Minimum Rust: 1.97** (`rust-version` in `Cargo.toml`). CI tracks `stable`, so this is a
declaration, not a pin; raise it only deliberately, after building on that version.

```bash
cargo build                       # build the workspace
make test                         # Rust tests + the Brood suite via cargo-nextest (capped run)
cargo run -p cli                  # start the REPL
cargo run -p cli file.blsp        # run a program file
cargo run -p cli -- --test f.blsp # run one self-contained test file
cargo run -p nest -- new foo      # scaffold a new project
make green                        # IS THE TREE GREEN? completed CI runs + the gates `make check` skips
make ci-wait [REF=sha]            # wait for CI's verdict on a push (follows cancellations) and name the failures
make green-all                    # …plus clippy on CI's flags, examples/stress gates, check-imaged, tier-audit, wasm-test
make ab BASE=<ref>                # A/B the working tree vs a git ref on the benchmark rows
make help                         # every target
```

- **After a push, `make ci-wait`**, never a hand-written `gh run list` loop; **`make green`**
  answers "is the tree green?", never a hand-read run list.
- **Clippy needs `--all-features`** (`cargo clippy --all-targets --all-features -- -D
  warnings`); every `unsafe` block and impl needs a `// SAFETY:` comment.
- **`make check-desktop`** runs CI's static gates on a machine that runs no suites;
  **`make affected-tests`** prints the tests that name the current change.
- **To ask what code does, `nest eval 'EXPR'`** (`--in MODULE` for a private name,
  `--test` for a scratch home, `-` for stdin): evaluating is not a test run, so it is fine
  here. Never a throwaway `tests/zz_probe_test.blsp`, which other sessions' runs then trip on.
- **Never benchmark on this machine**; `docs/benchmarking.md` has the rules for where you can.
- The reasons behind each, plus nextest, perf builds, the debug flags, the use-after-GC
  tripwire and verifier and the crash artifacts, are in
  [`docs/working-in-the-repo.md`](docs/working-in-the-repo.md). `brood --debug-flags` lists
  every `BROOD_*` flag.

## Working in this repo (the tree changes under you)

**Multiple changes happening at once is normal here.** The user edits files in parallel —
and sometimes renames or commits mid-task — so re-read before editing, and treat a
moved/changed file as the new reality, not an error to undo.

**Never run history- or state-altering git commands unless explicitly asked.** No
`git reset`, `git stash`, `git checkout`/`switch` to another branch, `git restore`,
`git rebase`, `git clean`, or force-push — any of these can silently discard the user's
concurrent work. Commit and push only when asked, and commit the state as it is. If the tree
looks inconsistent, surface it and ask.

**Work on `main`. Do not create a feature branch.** Commit straight to `main` and push there —
this overrides any default rule about branching. If `origin/main` has moved ahead,
`git merge --ff-only origin/main` first and **re-run the relevant tests on the combined tree
before pushing**.

**Before every push: `make hooks` once, then let the hook run (or `make prepush` by hand).**
The pre-push hook (`scripts/git-hooks/pre-push`) is the fast half of CI: rustfmt and the
`BROOD_*` flag catalogue always, plus greps for two of CI's Rust source gates (an unlisted
process-wide `static`, an unbound checker rule head); with a `.blsp` involved it first REBUILDS a
binary older than the tree (`PREPUSH_NO_BUILD=1` skips), then `nest format --check`, the tests
that name the changed modules, the surface audit and the executed doc examples (and, for a
prelude change, the bare-name ledger and the capture gate); for a change under `types/`, the
checker's expectation files. A red you did not cause is still yours to
attribute: run the failing file against a clean build of the previous commit.

> **A global `core.hooksPath` makes that hook INERT, and nothing says so** — it *replaces*
> `.git/hooks`. `make hooks` warns about it; `scripts/git-hooks/global-pre-push` is the
> chaining fix. On the development desktop that fix IS installed (`~/.config/git/hooks/pre-push`),
> so the repo hook runs on every push — **including its `.blsp` test step, which the
> no-tests-on-the-desktop rule forbids there.** On the desktop, run the hook's static gates
> with `make check-desktop` (fmt, `nest format --check`, `nest check` and `--strict` as CI
> spells them, clippy with `--all-features`) and push with `--no-verify`; CI runs the tests.
> Wherever you run `make prepush`, **read its exit status, not its last line**
> (`make prepush | tail` reports the pipe's status).

**Do not add a `Co-Authored-By: Claude` trailer (or any Claude/AI co-author attribution) to
commits in this repo.**

## Conventions & invariants (don't break these)

- **Proper tail calls are load-bearing.** The tree-walker (`eval/machine.rs`) is a loop over
  an explicit frame stack (ADR-416). When adding a special form with a body or branches, push a
  continuation (`KontKind`) for each non-last sub-form and hand the *last* form back to the
  loop (`self.expr = …; Ok(Step::Continue)`) — never a nested `eval`. The test
  `tail_calls_do_not_overflow` guards this.
- **All heap construction goes through the `Heap` constructors** (`alloc_pair`,
  `alloc_vector`, `list`, `alloc_string`, …; `value::sym` for symbols), never scattered
  constructors.
- **Prefer Brood over Rust** (ADR-006) — see the core principle above.
- **Favor the simplest user-facing design; defer power features** (ADR-011). Additive
  features cost nothing to delay; every knob is a tax on every user, forever.
- **Keep the language as small as possible.** Minimize special forms and evaluator
  semantics above all. When a feature can be a macro over a primitive function instead of a
  new special form, do that (`try`/`catch` is a macro over `%try`).
- **Symbols are interned `u32`s.** Compare with `==`; get the spelling via
  `value::symbol_name`.
- **Truthiness:** only `nil` and `false` are falsy (`eval::truthy`).
- **Brood data is immutable. This is absolute — do not weaken it** (ADR-026;
  `docs/language.md` §Immutability). There are **no data-mutation primitives** (no
  `set-car!`, `vector-set!`, atoms/cells/refs, **no `transient`/`assoc!`/`persistent!`**) and
  **none may ever be added** — not a transient, not a builder cell, not a mutable buffer, no
  matter how much it would speed up a build (a user-facing transient was tried and removed).
  Fast bulk construction is a **GC-quiet in-place build inside one Rust builtin** that still
  returns a fresh immutable `Value` (e.g. `%map-into`).
  - **An append buffer is not a mutable buffer** (ADR-511): a vector `conj`ed onto may share
    its element buffer with the vector built from it, which writes only past the length any
    existing vector reads, once per slot. Nothing that exists ever changes. It stays
    LOCAL-only (promotion, messages and clones copy out), and every write to a shared
    buffer goes copy-on-write (`VecStore::as_mut_slice`). Keep those two properties.
  - **The ONE and ONLY mutable structure is `Value::Table`** (shared, identity-mutable,
    deep-clones keys/values in and out). Reach for a `Table` or a **process** holding state
    in its loop when you genuinely need mutable state.
  - `let`/`fn` bindings never change. The only *binding* mutation is `def` rebinding a
    *global* (hot reload, ADR-013). There is **no `set!` and no `while`**.
  - **Assume immutability everywhere and keep the code simpler for it.** The kernel needs
    **no write barriers for data** — the minor GC relies on *old never points to young* (the
    sole remembered set is for `def`/env-frame *binding* rebinding). Don't add machinery
    (barriers, defensive copies) that only a mutable structure would require.
- **Types are set-theoretic, gradual, and advisory** (ADR-023/024; `docs/types.md`). A type
  *is* a set of runtime `Tag`s; subtyping is set inclusion; redefinable globals are
  `dynamic()`, never `Any`. The checker **never gates the live image and never warns on a use
  valid for the image's current state** — a `def`/reload always wins (ADR-123..126). The one
  hard reject is **batch/CI only** (`nest check` exits nonzero on any warning). Before adding
  a `Value` kind, primitive, special form or pattern, check the **compatibility contract** in
  `docs/types.md`. `sig`s are runtime contracts under `nest run`/`nest test` (ADR-381).
- **Runtime crates are allowed when they remove real complexity** — infrastructure that
  helps build the runtime (e.g. `boxcar` backs the shared RUNTIME code region), not
  Lisp-callable behaviour, which belongs in `std/`.
- **A runtime's inner processes share live code; separate runtimes don't.** All processes a
  runtime spawns share one `RuntimeCode` (`Arc`), so a `def` is visible to a running process
  on its next lookup — hot reload across processes. The prelude is a separate, immutable,
  shared-read-only region. Separate runtimes (nodes) stay independent. Data is *not* shared:
  each process has its own LOCAL heap; messages cross as deep copies (`docs/shared-code.md`).

## When you add a feature

1. Implement it (special form in `eval.rs`; a builtin in its `builtins/<domain>.rs` —
   implementation AND registration in the same file; or a prelude fn in `std/prelude/*.blsp`).
   **A prelude closure that calls a macro defined AFTER it is tree-walked for good** — define
   below the macros you use (`BROOD_DEFER_DBG=1` names a deferring closure).
2. Add tests — `(test …)` within a `describe` block in a `tests/*_test.blsp` file, opened
   with `(defmodule foo-test (:use test) (:use foo))` so the test macros and the module refer
   bare; and/or a Rust case in `crates/lisp/tests/`. **Every language feature must also be
   tested across multiple cores**: the in-language suite runs each test in its own green
   process, and any feature that produces or carries values needs explicit concurrency
   coverage — `spawn` workers that build the value, `send` it between processes (proving
   `to_message`/`from_message` and `promote`/freeze), read it from a shared global in many
   processes at once, fan in (see the `:isolated` "across processes" block in
   `tests/maps_test.blsp`). Prefer tail-recursive loops in test bodies; deep non-tail
   recursion raises a catchable `recursion too deep`.
3. **Sabotage every guard you write — a guard that cannot fail is worse than none, because
   it reads as coverage.** Break the fix, watch the test go red, restore. **Assert on the
   entry point a caller reaches, not on the piece you just wrote** (a flag the fix sets, a
   helper it added, a loop that runs before its inputs exist have all passed against the bug
   they were written for). When a change has no clean inverse to sabotage, find an argument
   from construction instead — do not settle for a weaker test.
4. Update `docs/language.md` (it documents the language *as implemented*).
5. Tick it off in `ROADMAP.md`; add a dated entry to `docs/devlog.md`.
6. If it reflects a real design choice, record an ADR at the bottom of the newest file in
   `docs/decisions/` (`docs/decisions.md` is the index).

## When you RENAME or REORDER a stdlib function

A rename wave or argument reorder is not a find-and-replace (ADR-302/308 cost ten bugs, six
silent). **Read the checklist in
[`docs/working-in-the-repo.md`](docs/working-in-the-repo.md#when-you-rename-or-reorder-a-stdlib-function)
before the first edit**: a call has five spellings, code generators sit outside every static
gate, the checker encodes argument positions structurally, `map` is also a type name, and a
rename wave ends downstream in bedit with a rename-ledger entry.

**Two measurement rules:**
- **Assert the summary line is PRESENT**, never that failures are absent — `grep "test
  failed"` is empty both on a pass and when the runner died, and a pipeline's exit code is
  the last command's.
- **Confirm the binary is fresh.** A killed `cargo build` leaves the old binary; the runtime
  says `this binary's baked-in std/ is OLDER than …` on stderr. `nest check` and
  `brood --check` refuse from a stale binary (exit 2); a test run only warns.

## Where things stand

The language core (M1), the editor data model (M2), the display protocol (M3) and the
server/daemon milestone (M4: TCP/TLS, `http/serve`, distributed nodes, `std/editor/serve`'s
daemon/`attach`) are delivered, along with the checker (zero warnings over `std/` + `tests/`,
deliberate lints opting out with `(check-allow :category form…)`), the GC, the package
manager, the self-hosted REPL, LSP, the closure-compiling VM + JIT and the WASM host. The
editor app itself is a separate downstream project. Open work is tracked in
`docs/handoff.md` and ROADMAP's "Still open" list.
