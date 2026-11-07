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

**When writing Brood code, read `docs/brood-for-claude.md` first.** It's the
pocket reference geared for AI assistants — syntax, idioms, and the patterns
that aren't shared with other Lisps. `nest new` also drops a copy into every
scaffolded project (it's baked into the binary via `%builtin-doc`).

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

```
crates/lisp/src/   (the directory tree mirrors the layers — see lib.rs)
  core/        substrate: value.rs (Value, Tag, symbol interner, Closure/Arity); heap.rs
               (per-process heap + shared regions, construction, alloc, region-dispatching
               accessors) with children heap/{gc (the tracing collector + verifier, split
               under gc/), gc_runtime, map_ops (CHAMP ops), equality, vm_cache (body cache,
               inline caches, shared JIT code cache), facts, positions, env_globals (env
               chain + global table), freeze (the once-per-runtime prelude freeze),
               promote (LOCAL -> shared RUNTIME copying), local_string, slabs, runtime_code
               (RuntimeCode: the shared `def`'d code + globals a runtime's processes hold
               in common), roots_buf}; alloc.rs, blob.rs, map_champ.rs, table.rs (the
               shared mutable table, ADR-107), sync.rs
  syntax/      reader, scanner, printer, and the tooling CST (atom / cst / scope)
  eval.rs + eval/   the tree-walking evaluator (machine.rs: a loop over a heap frame stack,
               ADR-416) + compile.rs/compile/,
               the closure-compiling VM (default engine, ADR-076): ir (IR types), lower
               (front-end), walkers, closure (compile_arm, shared body cache), emit
               (Node -> bytecode), prim_exec (the Prim2/Prim3 executors), dispatch (arm dispatcher, IC, JIT fast
               path), exec_chunk (bytecode inner loop), vm_run_bc (outer trampoline),
               inline (Node->Node passes), jit_plan (backend-independent lowering
               decisions), jit_runtime/ (tiering glue: compiler thread, link, dispatch,
               deopt), jit_lower (the Cranelift lowering); macros.rs (quasiquote,
               macroexpand) + macros/ (hygiene, resolve: namespaces and `compile`,
               pipeline and linmap: the loop rewrites, patterns: pattern lowering)
  types.rs + types/   the set-theoretic lattice, sig, display, and check/ (the advisory
               checker: walk/ dispatch, sigs, infer, and its tests)
  builtins.rs + builtins/   the primitive kernel, one file per domain; EVERY domain file
               owns both its implementations and its `register(&mut Primitives)`, so a
               primitive is a one-file edit. signature_types.rs holds the `Ty` shorthands;
               args.rs the argument readers (`arg`, `expect_int`, `expect_string`, …).
  boot.rs + boot/   the shared prelude bundle, the two boot paths (prelude image ADR-314 /
               source), boot/image.rs (the stdlib startup image, ADR-218)
  introspect.rs, cli_support.rs, renames.rs (the rename ledger, ADR-304), stack.rs (native
               stack growth over `stacker`), error.rs, lib.rs
  process.rs + process/   green-process scheduler (mailbox, message, monitor, links, timer,
               sysmon, io_source, pressure, cputime, receive_spec; scheduler/ split into
               pool, lifecycle, guards)
  dist.rs + dist/   distributed nodes (handshake, heartbeat, wire, session: the encrypted
               session ADR-089, guest: guest links ADR-415) — ADR-033/034
  host.rs + host/   feature-gated machine bindings, mechanism only: net (TCP/TLS, ADR-062),
               subprocess (ADR-104), wasm (`wasmtime` host, ADR-071/145), treesit,
               clipboard, gui + gui/ (ADR-046: winit backend, cosmic-text renderer, and
               gpu.rs the wgpu render target behind `gui-gpu`), audio, text_width
  diagnostics.rs + diagnostics/   coverage (ADR-148), perf/profile (VM counters, sampling
               profiler), debug_flags.rs (the `BROOD_*` catalogue behind --debug-flags)
  bundle.rs    single-binary app bundling (ADR-038) + the reserved `--brood-` argv prefix
  jit.rs + jit/   the JIT ABI + backend registry (ADR-101/221): backend.rs (the
               `JitBackend` contract), rt.rs (the `brood_rt_*` callbacks — the ONLY heap/GC
               interface native code has), cranelift.rs
  Module convention: a parent is `foo.rs` beside its `foo/` directory (never `foo/mod.rs`);
  children reach the parent's private items via `use super::*`.
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
  CI step behind it.
- **`make test` runs via `cargo-nextest`**: each test in its own process (a SIGSEGV is
  contained to one case), hard-capped per case (`.config/nextest.toml`). `cargo test` has no
  per-test timeout and can hang. `make ensure-nextest` installs it.
- **Perf-benchmark builds: `--bin brood` (or `-p cli`), never `-p brood`.** `-p brood` builds
  only the lib and does not relink the binary, so an A/B silently compares stale binaries.
  The JIT is a default feature (ADR-101). The tier ceiling is **`BROOD_TIER=0|1|2`**
  (tree-walker / bytecode VM / native, ADR-222) — the only engine knob.
- **Measuring performance:** `docs/benchmarking.md` §6 has the rules and why: `make ab
  ARGS=--floor`; `make ab-vm` for the VM call path; a base-vs-base control before believing a
  few-percent delta; re-run a tiering-sensitive row unpinned; measure a short and a long run;
  **count the calls** before optimising anything; attribute with `make perf-brood` +
  `(perf/summary)` (its own profile, `target/release-perf/`, never the timing binary). Run
  `make doctor` first — a stale binary fails by agreeing with the baseline. Never benchmark
  on this machine; see `docs/benchmarking.md` §7 for what this machine can still answer.

### Debug tooling

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
one. It catches Rust *panics*, **not** `SIGSEGV`; for those use
`gdb --batch -ex run -ex bt <test-binary>` (`rr` isn't installed, and `valgrind` won't see a
*logical* use-after-GC over safe `Vec` slabs).

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
> by hand (`cargo fmt --check`, `nest format --check`, `nest check` and `--strict` as CI
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
