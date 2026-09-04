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

Brood source files carry the **`.blsp`** extension — a contraction of *Brood
Lisp* (`.lisp` was dropped because it collides with Emacs' `lisp-mode`). Any
`.blsp` file, or a reference to "blsp", means **Brood-language source** (the
language itself), as distinct from the Rust kernel under `crates/`.

**When writing Brood code, read `docs/brood-for-claude.md` first.** It's the
pocket reference geared for AI assistants — syntax, idioms, and the patterns
that aren't shared with other Lisps. `nest new` also drops a copy into every
scaffolded project (it's baked into the binary via `%builtin-doc`).

## Greenfield: prefer the right structure over compatibility

This is **greenfield** — pre-1.0, no external users, nothing to keep stable.
**Make breaking changes freely when they improve the overall structure.** Don't
add compatibility shims, deprecation aliases, or "keep the old way working too"
hedges; rename, re-shape, or delete the old thing and update every caller. A
clean, coherent design beats a backwards-compatible one every time here. (Keep
the build/tests green, and record notable breaks in `docs/devlog.md` — but don't
preserve a worse design just to avoid a break.)

## Green tree first — don't start new work over known flakiness or bugs

**Before picking up any new feature or perf work, the tree must be provably green.**
A known open bug, a flaky test, or an unexplained failure — *even one seen "only
once"* — **is** the work: fix it, or prove it a non-issue, before building anything on
top. A feature layered over a latent bug only makes the bug harder to find, and a flake
left running erodes trust in every green run after it (you stop believing your own CI).

Before starting new work:
- **Confirm no open issue.** `docs/known-issues.md` should show no open bug. A documented
  *watch* item is acceptable only if it genuinely cannot be reproduced on demand — say so,
  with the diagnostic armed.
- **Prove green, not "passed once."** Flakiness hides in one-in-N races, so a single pass
  is not evidence. For anything touching concurrency, the scheduler, dist/nodes, GC, or the
  JIT, run the suite *repeatedly* — `make test` and `make test-both`, the distribution tests
  looped, `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` on the concurrency-heavy tests, and a
  fuzz-differential pass (`scripts/fuzz/run.sh`). The flake-hunt tooling is below and in
  `docs/handoff.md`.
- **Gate performance at the tier your change runs on.** `make ab` measures the default
  ceiling, where a hot arm is native and the interpreter's call path never executes — so a cost
  added to `exec_chunk`/`dispatch`/`vm_run_bc` reads as *flat* there. KI-40 was a 3.19x
  regression that `make ab` reported as +1.3%. Use **`make ab-vm`** (ceiling 1) as well for
  anything on the VM's call path, and remember `make doctor` first: a stale binary fails by
  agreeing with the baseline.
- **A regression is stop-the-world.** If new work turns a green row red or a green test
  flaky, fixing that takes priority over finishing the feature — don't commit forward.
- **Build uncapped, run capped.** Put an address-space cap in front of every test, `nest check`,
  `nest run` (in a downstream project too — bedit is where KI-87 first showed) and `brood`
  run — `( ulimit -v 16000000; cargo nextest run -p brood -j1 -E '…' )` — and keep `-j1`, so
  one runaway is the whole spike. **16 GB, not 4:** the runtime *reserves* ~3 GB of address
  space before it does any work on a 28-core box — the allocator's per-thread arenas (20 ×
  128 MB `PROT_NONE`) plus the worker stacks (28 × 16 MB) — so a 4 GB cap fails the first
  test that creates a `table` (its 64 MB virtual region is merely the mapping that lands on
  the wall; measured 2026-08-30 with `strace -e mmap`), and that failure is a Brood error
  naming the cap, not a runaway. 16 GB still catches the KI-87 class (19 GB processes).
  **The wasm exception is GONE (2026-09-04) — do not re-add it.** `tests/wasm_sandbox_limits_test.blsp` used to fail under the cap even run alone, and `tests/wasm_test.blsp` intermittently beside it, so a capped run reporting those two was written off as "the cap, not a regression" — a standing exception that would have hidden a real sandbox regression. The cause was wasmtime RESERVING 4 GiB of address space per linear memory: eight small memories asked for 32 GiB and the sandbox reported the refusal as denying a module it is documented to allow. `crates/lisp/src/wasm.rs` now sets `memory_reservation(MAX_GUEST_BYTES)` — never reserve more than `GuestBudget` will ever let a guest use — and both files pass capped (7/7 and 15/15). A capped run that reds a wasm file is now a real failure. A *diverging* process is indistinguishable from a
  heavy one until it has eaten the machine: KI-87 (a checker cycle guard that un-guarded) put
  three test processes at 19 GB each and a `nest run` at 54 GB, crashing the box three
  sessions running; under the cap the same runs die in ten seconds with the panic site named
  (`stacker … mmap failed to allocate stack`). **Never put the cap in front of a build**
  (`cargo build`, `nextest --no-run`) — the linker inherits it and dies with `LLVM ERROR: out
  of memory`. Build first, then run under the cap.

A runtime whose green runs are trusted is worth more than one more feature; every later
hour of "is this flake real?" costs more than proving green up front.

## Core principle: write the language in the language

**As much of the system as possible must be written in Brood itself, not in
Rust.** This is the most important rule in this repo — it is the entire reason
the project exists (a self-editing editor is only possible if its behaviour
lives in code you can redefine at runtime).

Concretely:
- Rust provides **mechanism**; Brood provides **policy**. Use Rust only for
  what genuinely needs it: primitives the language can't bootstrap (low-level
  I/O, the rope/text engine, performance-critical kernels) and the core
  evaluator itself.
- Everything else belongs in `std/` (Brood source), not in `builtins.rs`. When
  you reach for a new Rust builtin, first ask: *can this be written in Brood on
  top of existing primitives?* If yes, do that instead.
- This applies to upcoming pieces too. The **editor commands, keymaps, and UI
  should ultimately be Brood**, with Rust only hosting the thinnest necessary
  substrate. (The REPL already moved into Brood — `std/tool/repl.blsp`, ADR-048;
  the binaries just bootstrap into `(repl-run)`. The `nest`/`brood` CLI dispatch
  is still Rust — see `ROADMAP.md`.)
- A Rust builtin is an admission that the language can't yet express something.
  Treat each one as a candidate to later replace with Brood once the language
  is capable enough.

### Dogfood first; optimize only by building the language up, not around it

**Keep code in Brood even when it's slower, so we surface the language's real
gaps.** The more of the system that runs on our own functions, the more we learn
which primitives and capabilities are actually missing — that feedback is the
whole point of a self-hosted language. Reaching for Rust to make a slow Brood
function fast *hides* the gap instead of fixing it.

So before optimizing, the change must satisfy **both**:

1. **It improves overall language performance** — a capability that pays off
   broadly, not a one-off speed-up for a single call site.
2. **It builds up the right primitive/capability** — it makes the *language*
   more capable (so Brood code gets faster), rather than moving behaviour out of
   Brood into a Rust escape hatch.

Worked example: variadic `+`/`-`/`=` (Brood `defn`s over `fold`) cost ~40× a
direct call. The wrong fix is making them Rust builtins (fast, but reverses
"write it in Brood" and teaches us nothing). The right fix is giving the
*evaluator* efficient **multi-arity dispatch** — a general capability that keeps
`+` in Brood, makes *every* multi-arity function faster, and is exactly the kind
of primitive dogfooding revealed we were missing.

This bar may relax once the language is more stable and we deliberately tune hot
paths — but until then, prefer learning over shortcuts.

## Layout

```
crates/lisp/src/   (the directory tree mirrors the layers — see lib.rs)
  core/        substrate: value.rs (Value, Tag, symbol interner, Closure/Arity),
               heap.rs (per-process heap + shared regions + env chain: construction,
               source-positions, definition-sites, alloc, accessors, env-chain, globals)
               with child modules heap/{gc.rs (roots/collection/RUNTIME-compaction/stats),
               map_ops.rs (CHAMP ops), equality.rs (equality/compare/hash),
               vm_cache.rs (VM body cache + inline caches)} — children of `heap`, so they
               reach Heap's private items via `use super::*`, alloc.rs,
               blob.rs (cross-process zero-copy blob heap), map_champ.rs (CHAMP
               map trie), table.rs (shared mutable table — Brood's ETS, ADR-107), sync.rs
  syntax/      reader.rs (text -> Value), scanner.rs, printer.rs, and the tooling
               CST (atom.rs / cst.rs / scope.rs)
  eval/        mod.rs (evaluator — a `'tail: loop` for tail calls + special forms),
               compile/ (the closure-compiling VM — the default engine, ADR-076),
               split into files (all child modules of compile/mod.rs — `use super::*` +
               `pub(crate) use child::*`):
                 - ir.rs — IR types: PrimOp/PrimOp1, ConstVal, Node, CompiledArm/Closure, Chunk/Inst
                 - mod.rs — the compiler front-end (compile_arm, compile_node) + shared
                   IR walkers + run/apply entry points + BcFrame/Suspended
                 - emit.rs — emit_node/compile_chunk (Node → bytecode)
                 - exec_value.rs — exec_value + prim exec helpers (Node tree-walk)
                 - dispatch.rs — the VM arm dispatcher (Call/SelfCall, IC, JIT fast path)
                 - exec_chunk.rs — the bytecode interpreter inner loop
                 - vm_run_bc.rs — the outer VM trampoline (tail-call loop, frame save/restore)
                 - inline.rs — Node→Node optimizer passes (linmap rewrite, self/leaf inlining)
                 - jit_plan.rs — backend-INDEPENDENT lowering decisions (ADR-221): frame
                   layout (jit_spill_reserve/jit_ckpt_depth, ungated — the VM sizes frames
                   with or without a JIT) + a `codegen` submodule, gated once, for the
                   subset rule, the profitability gate (plan_general_lowering/BailReason),
                   LICM analysis and the alloc predicates
                 - jit_runtime.rs — JIT tiering glue (feature = "jit")
                 - jit_lower.rs — jit_lower_arm / jit_lower_arm_inner: the Cranelift
                   lowering, i.e. `jit::JitBackend`'s implementation (feature = "jit").
                   Lives under `compile/` rather than `jit/` because it reads compile's
                   private IR; it decides nothing — see jit_plan.rs
               macros.rs (quasiquote, macroexpand, the compile pass + pattern lowering)
  types/       mod.rs (Ty/GradualTy set-theoretic lattice), check.rs + check/
               (advisory checker)
  builtins/    functions implemented in Rust (the primitive kernel); split into:
               mod.rs (Reg struct, pub fn register, PRIMITIVE_DOCS, shared helpers),
               numeric.rs (numeric/bitwise/bitset/math), sequences.rs (pair/list/range/
               seqview/vector/map/string/rope), io.rs (TCP/table/print/time/fs/hashing/
               git/crypto), os.rs (env/hostname/os-cmd/run-process/halt), terminal.rs
               (terminal + GUI, feature-gated), system.rs (eval/load/processes/dist/
               dynamic/namespaces) + selfhost_macros.rs (macroexpand/check) + tooling.rs
               (source-positions + introspection, editor/LSP) + errors.rs (throw/try).
               All submodules use glob re-export (`use X::*`) so register() is untouched
  introspect.rs  doc/arglist/global-names/bound? and friends (ADR-025)
  cli_support.rs file-runner / --test plumbing shared by the binaries
  process.rs + process/   green-process scheduler (mailbox, message, monitor, links,
               timer, sysmon, io_source) with the scheduler itself split under
               process/scheduler/ (pool, lifecycle, guards): spawn/send/receive/monitor
  subprocess.rs   persistent child-OS-process mechanism (ADR-104) — distinct from
               process.rs (green processes); renamed from proc.rs to end the name clash
  dist.rs + dist/   distributed nodes (handshake, heartbeat, wire) — ADR-033/034
  net.rs       thin non-blocking TCP socket mechanism (ADR-062); Brood policy is
               the in-tree `std/net/*` library (net_wasm.rs is the wasm32 shim)
  wasm.rs      embedded `wasmtime` host — `%wasm-*` load/call/exports, WIT-typed
               lower/lift + fuel metering (ADR-071/145; Brood policy `std/wasm.blsp`)
  treesit.rs   tree-sitter integration for the editor highlighter (std/editor/treesit.blsp)
  bundle.rs    single-binary app bundling (ADR-038) + the RESERVED `--brood-` argv
               namespace a bundle honours in first position (ADR-257): `--brood-build-info`
               and `--brood-boot-check`. Everything else in argv is the app's;
               gui.rs the GUI frontend (ADR-046);
               gui_gpu.rs the experimental OpenGL backend; audio.rs `audio-beep`
  jit/         the JIT's ABI + backend registry (feature = "jit", ADR-101/220):
               backend.rs (the `JitBackend` contract — six obligations a backend must
               satisfy), rt.rs (the `brood_rt_*` callback table, the ONLY heap/GC interface
               native code has — backend-independent), cranelift.rs (`CraneliftBackend`:
               the Cranelift `JITModule` owner + the impl), mod.rs (sentinels +
               `ActiveBackend`). The lowering itself is `eval/compile/jit_lower*`
  coverage.rs  line-coverage instrumentation (ADR-148); perf.rs/profile.rs VM counters;
               debug_flags.rs the `BROOD_*` catalogue behind `brood --debug-flags`
  error.rs     LispError / LispResult / source Pos
  lib.rs       the `Interp` entry point; bundles std/prelude/*.blsp (concatenated in order)
crates/cli/src/main.rs   the `brood` binary — the language (REPL, file runner, `--test`)
crates/nest/src/         the `nest` binary — project tooling (main.rs + mcp.rs) — ADR-028
crates/lsp/src/main.rs   the `brood-lsp` binary — language server (ADR-025, docs/lsp.md)
crates/playground/src/   the `brood-playground` cdylib — a wasm-bindgen shim exposing
                         a Brood `eval()` to the browser (the in-browser playground)
std/                     standard library written in Brood, grouped (ADR-085):
                         NOTE: `bit`, `decimal` and `proc` also namespace KERNEL
                         primitives (ADR-251) — the `.blsp` file declares the module
                         and documents the surface; the ops are Rust, registered as
                         `bit/and`, `decimal/of`, `proc/register`, the way
                         `string/length` always has been.
                         prelude/ (split across ~9 bare-root files (core, predicates, map, …), concatenated in the order lib.rs lists) + ~30 bare-core modules (io, file, set, regex,
                         json, format, task, log, version, resolver, crypto, hash, csv,
                         datetime, encoding, url, uuid, template, stream, …); the
                         perf-triage module `std/tool/perf.blsp` (`perf/report`,
                         `perf/summary`, `perf/measure` — DEV); the
                         editor/display framework `std/editor/*` (buffer, display, ui,
                         keymap, face, highlight, lineedit, pane, layers, ansi, serve);
                         the process framework `std/proc/*` (`gen`, `supervisor`,
                         `agent`, `crash-report` — the default crash reporter, ADR-305); the net *library* `std/net/*` (`http`, `sse`, `tcp`,
                         `reconnect`); the toolchain `std/tool/*` — all four grouped on
                         disk but with BARE (single-segment) module names, so a qualified
                         call is `gen/spawn-server`, `http/get`, `test/run` — never a
                         double slash (test, project, package, complete, coverage, debug,
                         docs, eval-server, explain, grammar, mcp, observer, proctree,
                         repl, scaffold, sexp, reload). `std/editor/*` is the one
                         exception — it keeps the `editor/` prefix (a cohesive framework
                         whose names, `buffer`/`ui`/`pane`/`ansi`, are deliberately generic
                         and would collide/land-grab bare; `ansi` already exists top-level). The
                         net library and `supervisor` were briefly externalized (Move 2)
                         then re-bundled in-tree (ADR-097, batteries-included default);
                         the Rust socket *mechanism* stays in-tree too
                         (`crates/lisp/src/net.rs`, ADR-062). The REPL is Brood too
                         (`std/tool/repl.blsp`, ADR-048); the binaries bootstrap
                         into `(repl-run)`.
docs/                    architecture, language, roadmap, decisions, devlog,
                         handoff.md (current state + open threads — read first when
                         resuming work; measurement traps are listed there)
```

The CLI is split (ADR-028, the `rustc`/`cargo` model): **`brood` runs the
language**, **`nest` runs the project**. Both embed the `brood` lib (no
subprocess); `nest` is a thin shell over `std/tool/project.blsp`. `nest` subcommands
today: `new`, `test`, `check` (with `--fix-renames`, which applies the *unambiguous* half of
a rename wave's recovery, consulting the rename ledger first — ADR-304), `run` (refuses to launch over an *unbound symbol* in the entry point's require-closure — the checker already ran there and bedit ignored it for hours; `--no-check` skips the pre-flight, ADR-304 — with `--watch`, and `--check-boot`: load every
module, resolve `:main`, run **nothing**, exit nonzero — the question `check` and `test`
both leave unasked, KI-66), `doc`, `format`, `repl`,
`mcp` (an MCP server over the project), `observe` (the M3 process viewer),
`attach` (the `emacsclient`-style thin frontend for a daemon serving a `ui-run`
app — ADR-090), `completions` (emit a shell TAB-completion script; `complete` is the hidden
candidate engine behind it), `grammar` (emit an editor syntax grammar — VS Code TextMate or
Emacs — generated from `(special-forms)`, ADR-092), the package-manager commands
`fetch`/`update`/`tree`/`add`/`remove` (ADR-037) plus `publish`/`search` against a
**hosted** registry (the `hive` tarball service, ADR-147/211 — *not* a git-backed
index) and `key` (generate/manage the ed25519 signing keypair `nest publish` uses,
ADR-212), `release` (single-binary bundling, ADR-038; boot-checks each binary it just wrote — the
*artifact*, which carries a dependency snapshot no source-tree check can see — and **deletes
it + fails** if it does not boot. On by default since 2026-08-29; `--no-smoke` opts out), and `update-tooling` (re-drop
the AI-assistant files `nest new` scaffolds — the `docs/brood-for-claude.md`
reference and the `writing-brood` skill — from the current binary, so they don't
drift as the language evolves).

## Commands

**Minimum Rust: 1.95** (`rust-version` in `Cargo.toml`, inherited by every crate). CI tracks
`stable` deliberately, so this is a *declaration*, not a pin — but it is the difference between
cargo naming the version it needs and a bare `error[E0658]: use of unstable library feature`,
which reads as broken code rather than an old toolchain. Raise it only deliberately, and only
after building on the version you claim.

```bash
cargo build                       # build the workspace
make test                         # Rust tests + the Brood suite via cargo-nextest
cargo test                        # same, but plain libtest — NO per-test timeout (can hang)
cargo run -p cli                  # start the REPL  (or: ./bin/cli)
cargo run -p cli file.blsp        # run a program file
cargo run -p cli -- --test f.blsp # run one self-contained test file
cargo run -p nest -- test         # discover + run the project's test suite
cargo run -p nest -- new foo      # scaffold a new project
make ab BASE=<ref>                # A/B the working tree vs a git ref on the benchmark rows
make green                        # IS THE TREE GREEN? completed CI runs + the gates `make check` skips
make green-all                    # …plus clippy on CI's flags, the examples/stress gates, and `check-imaged`
```

**`make green` is the answer to "is the tree green?" — do not hand-read the run list.** It
exists because that question was answered wrongly for two days (KI-68/KI-69), twice, and
neither reading was careless. First, **a cancelled CI run is not evidence**: every run is
cancelled by the next push to the ref, so a busy day shows a wall of `cancelled` with an
`in_progress` on top and *no red anywhere*, while the last several **completed** runs were all
failures. Second, **`make check` is not what CI runs** — it is clippy + tests, and CI also runs
`nest format --check`, the zero-warning checker gate over `std/` + `tests/`, the examples gate
and the stress gate; v0.14.0 was tagged and pushed with `nest format --check` red for exactly
that reason. `make green` reports the completed runs of the **CI workflow specifically** (the
`Release` workflow goes green on commits whose CI failed) and runs the local gates `make check`
omits; it prints **"no verdict"** rather than "green" when CI has concluded nothing recent. CI also runs a **`downstream-bedit`** job — bedit pinned to `BEDIT_REF` in the workflow, `nest check` + `nest run --check-boot` + `nest test` against the commit under test, the gate the ADR-302 rename wave showed was missing — and `make smoke-bedit` (in `make green-all`) is the same three commands against `../bedit` locally.

> **Clippy: `--all-features` is load-bearing, not thoroughness.** CI runs
> `cargo clippy --all-targets --all-features -- -D warnings`; **a plain
> `cargo clippy --all-targets` passes on code CI rejects**, because the smaller feature set
> arms fewer lints. That cost two commits on 2026-08-27, and the second was expensive out of
> proportion: the `Clippy` step failing **skips every step behind it**, so the tests, the
> doctests and the checker gate never ran on that commit at all. `make green` says clippy was
> not run and prints the exact invocation; `make green-all` runs it.

**Reading where the time goes: `make perf-brood`, then `(perf/summary)`.** The counters are a
cargo feature, so an ordinary (or installed) binary cannot attribute at all — `make perf-brood`
is that build, with `release-brood`'s exact flags plus `perf-stats` so it cannot drift from what
you compare it against. A qualified call then loads it — `require` is not a bound name; a `mod/name`
reference infers the load — so `(perf/report)` gives a map, `(perf/summary)`
(printed), and **`(perf/measure thunk)`** — use the last one for anything narrower than a whole
process: the counters are cumulative from process start, and boot is expansion-heavy, so the
same program read an **84% tree-walker defer rate on a cold boot cache and 0.8% on a warm one**.
`(vm-stats-reset)` is the primitive underneath. Two things `perf/report` deliberately will not
tell you: it never concludes `:alloc-bound` (once an arm goes native its iterations stop being
counted while its allocations do not — a 200k-iteration loop measured `:alloc` 200017 against
`:vm-apply` 197), and it refuses to judge any rate resting on under 1000 samples (0 hits of 12
IC probes is not a 0% hit rate). See `docs/backend-seams.md` §5.

**`make perf-brood` OVERWRITES the binary `make release-brood` builds — same path.** Both
write `$(RELEASE_DIR)/brood`, so the moment you profile something, `target/release-fast/brood`
is the **counter-armed** build, and timing it against an `ab` worktree's clean build charges
your change ~10% for atomics it never introduced. This needs no command-line slip: profile,
then time, and the bias is there. It reads as a plausible regression — on 2026-08-27 it showed
two *behaviourally identical* binaries (working tree reverted to HEAD) at 958 vs 1018 ms.
**Rebuild with `make release-brood` before any timing**, and prove it with a base-vs-base
control: identical inputs must measure identically before a delta means anything. Relatedly,
timing whole `brood` invocations in a shell loop measures the COLD-tier path every time and
never reaches the warm steady state — time rounds *inside* one process to see both arms (a
JSON round-trip: round 1 ~300 ms, warm ~224 ms).

**Measuring a perf change: use `make ab`** (`scripts/ab-bench.sh`), don't hand-roll
it. Add **`--floor`** when a row moves by only a few percent: it runs the baseline twice and
reports that row's own base-vs-base spread, and the `verdict` column then calls a regression
only when the delta clears `max(5%, 2 x floor)` — the discipline described further down, applied
automatically. **`--json <path>`** writes the sweep as machine-readable rows. It builds the baseline ref in a throwaway worktree and the working tree
through the *same* `make release-brood` target, so profile and features cannot
drift between the two sides; then it warms each binary's build-id-keyed boot
cache, pins with `taskset`, and reports best-of-N per row against
`../brood-benchmarks/bench/brood/*.blsp`. It aborts if the two binaries come out
byte-identical, which is what a silently no-op'd build looks like. Examples:
`make ab BASE=HEAD~3`, `make ab N=11 ROWS="fib pfib"`, `./scripts/ab-bench.sh --all`
for the regression sweep, `--list` for the row names, `make ab-clean` to drop the
worktrees. It measures brood against brood — the *published* cross-language
numbers come from `bench/harness.py` in `brood-benchmarks` (all seven languages in
one session; see that repo's `CLAUDE.md` for the publish order).

**Installing for a published benchmark run: always install the LEAN build** —
`make install INSTALL_FEATURES='$(RUN_FEATURES)'`. That harness runs whatever
`brood` is on `PATH`, and a plain `make install` adds `brood/dev-tools` (the REPL,
`nest test`, the observer, the MCP server) — developer tooling that is not part of
the runtime an app ships. The reason is **build consistency, not a startup cost**:
`RUN_FEATURES` is already what `make ab` measures and what `nest release` embeds,
so installing lean keeps the published numbers, the A/B numbers, and what users run
on one build. (Measured 2026-07-29, same commit, best-of-9: lean and dev-tools
startup are identical — 10 ms / 18.8 MB each — because the DEV_MODULES are
`require`d on demand, not baked into the boot image. Don't repeat the plausible
claim that dev-tools inflates `startup`/base-RSS; it doesn't.) It also makes the
installed `nest` lean, so reinstall with the plain `make install` when you want
`nest test`/`repl` back.

**`make ab` pins compute rows to ONE core, which charges the benchmark for background
JIT compilation.** That is right for measuring generated-code quality and wrong for any
change that alters *how much* the background compiler does: the compiler thread competes
with the benchmark for the single pinned core, so extra compiles read as a regression that
a real (multi-core) run never pays. Found 2026-07-29 — ADR-175's shared compiled code
makes more prelude arms tier up (18 lowered vs 7), which showed as `collatz` +8% pinned
and **zero unpinned**. If a change touches tiering, compilation volume, or anything the
background compiler does, re-run the row **unpinned** (`taskset` off) before believing a
regression. `BROOD_JIT_DUMP_IR=1 … | grep -c '^\[jit-ir\]'` counts the compiles.

A row that moves only a few percent in a sweep deserves a solo re-run before you
believe it: interleaving A and B shifts thermal/cache state, and on 2026-07-27
`persistent-map` read +3.7% in a sweep and 82 vs 81 ms measured directly.

**A solo `make ab` re-run is not always enough — some rows drift between whole
invocations.** On 2026-07-29 `pingpong` read +4.3% in a sweep and **+5.3% solo**, which
looked like a confirmed regression; its `make ab` *baseline* had meanwhile wandered
209 → 230 ms across the day's runs (~10%), so the "confirmation" was measuring drift
twice. The reliable method for a suspect row is a **fixed baseline binary + a base-vs-base
control**: keep the `target/ab/<sha>/…/brood` binary, run `taskset`-pinned best-of-15 for
base, base again, then new, and read the base-vs-base spread as that row's noise floor.
The same change then measured +0.9% against a +0.5% floor — neutral. Don't report a
regression whose size is within a couple of multiples of the floor you haven't measured.

**Measure BOTH a short and a long run — a tiered runtime has two different steady
states, and a micro-benchmark only ever reports one of them.** The JIT tiers up in the
background, so the same code path can be interpreted, natively compiled, natively
*inlined*, or deopted-and-bailed depending only on how many times it has run. A verdict
from one call count is a verdict about that call count:

- **Short** (thousands of calls): the interpreted / not-yet-tiered path. This is what
  real short-lived work actually pays — a `nest check`, a one-shot script, a request
  handler on a freshly spawned process — so it is not merely the "unwarmed" case to be
  discarded.
- **Long** (millions of calls): the fully-tiered path, including the deferred inlined
  upgrade (two-stage tiering means the *second* body arrives later than the first).

Sweep the call count across at least two orders of magnitude and check whether the
**gap between the two arms moves**, not just whether each arm gets faster. A mechanism
can be neutral warm and a win cold (or the reverse), and either way reporting one number
hides it. Brood's tiering is fast — the crossover is usually well under a million calls —
so this is cheap to check and there is no excuse for skipping it. Cross-check with
`BROOD_JIT_DUMP_IR=1 … | grep -c '^\[jit-ir\]'` (did the arm ever lower?) and
`BROOD_DEOPT_TRACE=1` (did it lower and then fall back?), and remember that a bailed arm
never reaches the IR dump, so *absence* there is the signal.

**Before optimising anything, COUNT THE CALLS. A code-reading is evidence about what a
function does, never about how often it runs.** `compute-frontier.md` §7.8's top-ranked
item was "confirmed by reading the cited code" and described a verdict "recomputed per
activation, behind a global `Mutex`". It was implemented, measured, and reverted: `make ab
--floor` read noise on every row, and a five-line probe then showed the function is called
**21 times on `fib`**, 457 on `ackermann`, 2176 on `pfib` — against billions of
activations. One `&&` above the call short-circuited it. The read was accurate and the
conclusion was wrong.

So: a `static AtomicUsize` and an `eprintln!` at the call site, one run, *then* decide. It
costs minutes where a build-then-A/B round trip costs hours, and it is the only cheap way
to tell a hot path from a plausible one. The same probe answers the follow-up question a
profile does not — `perf record` names cost centres in the code you *have*, while the count
tells you whether the code you are *about to write* would ever run.

Cargo is the source of truth; a thin **`Makefile`** wraps the common commands as
shortcuts (`make help` lists them): `make build`, `make test`, `make suite`,
`make repl`, and `make benchmark`. **`make test` runs the suite via
[`cargo-nextest`](https://nexte.st)** — each test runs in its own process (so a
SIGSEGV from a green-process stack overflow is contained to that one case, not the
whole binary) and is **hard-capped at 2 min** (`.config/nextest.toml`), so a hung
test is killed on its own and the run still finishes. Get it with
`make ensure-nextest`. The last runs the `divan` benches
(`crates/lisp/benches/`) via `scripts/bench.sh`, which archives each run with full
environment metadata to `docs/benchmarks/<UTC-timestamp>.md`. `make -j$(nproc)`
parallelism isn't relevant — it's a Cargo workspace, not a recursive make.

> **Perf-benchmark builds: use `cargo build --release --bin brood`, NEVER `-p brood`.**
> The **JIT is now a default cargo feature** (ADR-101), so an ordinary build already includes
> it — `--features jit` is redundant (harmless, and still needed only under
> `--no-default-features`). The `brood` *binary* is the `crates/cli` package; `-p brood` builds
> only the `crates/lisp` *lib* and does **not** relink `target/release/brood`, so an A/B with
> `-p brood` can silently compare **stale binaries** (this produced a fully bogus 8B go/no-go +
> a phantom "JIT regression" on 2026-06-18 — see devlog). Always `--bin brood`, and sanity-check
> binaries differ (size/mtime) before trusting an A/B. Engines: `BROOD_VM=0` = tree-walker
> (legacy, ~10× slow); unset/`BROOD_VM=1` = bytecode VM; the tier-1 JIT tiers *within* the VM
> path — disabling it means `BROOD_NO_JIT=1` (runtime) or a `--no-default-features` **build**.

### Debug tooling — knobs, env flags, and crash artifacts

The kit for chasing GC / use-after-GC and other kernel faults. **Build with
`RUSTFLAGS="-C debug-assertions=on" cargo build --release`** to keep release
speed while arming every debug check below (plain `--release` strips them for
zero shipped cost; plain `cargo build` debug is correct but too slow to expose
contention races).

| Env flag | Effect |
|----------|--------|
| `BROOD_GC_STRESS=1` | Collect at **every** eval safepoint (not just when the threshold is crossed). Turns rare GC races into deterministic ones. |
| `BROOD_GC_VERIFY=1` | **Heap verifier** (now works in plain `--release` too, gated by the flag): before each collection, walk the whole reachable LOCAL graph and assert every handle is in-bounds + current-epoch. Catches a *stored* stale handle (the use-after-GC class the per-deref tripwire misses — a bad handle written into a heap cell, e.g. into game state) and prints the `root→…→cell` path at the store site's next collection. O(live) per collection only when set. See below. |
| `BROOD_TRACE_GCBLOCK=1` | Trace GC-block depth (debug). |
| `BROOD_TRACE_PROMOTE=1` | Name every closure entering the **append-only RUNTIME region**, with the Rust frames that put it there (`[promote] closure <name> :: <frames>`). The region only grows, so anything promoted *per operation* is an unbounded leak of shared code — and the symptom is a slow throughput decay, not a crash. Pipe through `sort \| uniq -c \| sort -rn` to rank the sites. This is what pinned thread 6 after hours of elimination bisecting had failed: 1382 of 1389 promotions in a supervisor workload came from one site, `spawn_impl <- spawn_link`. Works in release; costs one `var_os` when off. |
| `BROOD_SCOPE_DBG=1` | Name every process still **live when `%isolate` rolls the globals back** (`[scope] RESTORE by <pid> (scope=N) with K live: …`, each survivor with its isolate stamp and parent). The KI-89 tool: an isolate is sound only while nothing else is mutating globals, and nothing said when that was violated — the failures land in *other* files, so they never named the cause. One run of this made the shape readable (a worker rolling the table back while three siblings executed) where a week of inference had not. Pair with `BROOD_REG_TRACE=1`, whose `owner=` field says which isolate a registry writer was spawned under. Works in release; one `var_os` per isolate when off. |
| `BROOD_REG_TRACE=1` | Trace every `*record-ids*` registry write — with the writer's 4-hop **ancestry chain**, so a leaked writer is attributable after its parents exited — plus every globals-snapshot restore (`[reg] … RESTORE`). The KI-89 orphaned-record-id attribution tool: read the seven id writes' `chain=` against the RESTORE lines. Deliberately narrow — the first version traced *all* registry writes and the added stderr-lock serialization **suppressed the race being hunted** (0 orphan runs traced vs 3/3 untraced on the same binary), so keep it lean. One cached `var_os` when off. |
| `BROOD_COVERAGE=1` | Arm **line-coverage** instrumentation (ADR-148 tier 2): the compiler prefixes each positioned node with a `RecordLine` opcode, so `%coverage-lines` / `%coverage-instrumented` can report which executable lines ran. Read **once and cached**, so it must be set before anything builds an `Interp` (the prelude compiles during construction) — set late it silently instruments nothing. `nest test --cover-lines` sets it (plus `BROOD_NO_JIT=1`) in `main`. |
| `BROOD_MEM_LIMIT=<bytes>` | Arm the ADR-043 soft/hard memory cap for a run. |
| `BROOD_STACK_BUDGET=<bytes>` | Raise/lower the non-tail-recursion stack guard. |
| `BROOD_RT_GC_FLOOR=<count>` | Threshold floor (RUNTIME closures) for reclaiming the shared code region — single-process compaction when uniquely owned, else the unconditional 2-generation collector (ADR-091; default 4096). The shared-region counterpart of `BROOD_GC_FLOOR`. |
| `BROOD_TRACE_COMPILE=1` | Name every closure the VM **bytecode-compiles**, with its cache key and body region (`[compile] id_region=… key=Body(…) body=…`). The question it answers: is a compiled body being *reused*? A key repeated once per process means the shared cache is missing — which is exactly how ADR-215 was found (100 154 compiles for 100 000 spawned processes, 8.1 µs each). Works in release; compiles are rare once sharing works, so the check is free. |
| `BROOD_NO_SHARED_ARMS=1` | **Opt-OUT** of runtime-shared compiled code (ADR-175 + ADR-215): every process compiles its own copy of every closure it calls, as before. The A/B lever and the bisect switch for a suspected stale-shared-arm fault; `spawn-live` costs 25% more CPU and 14% more RSS with it set. |
| `brood --debug-flags` | Not a flag — **the list of them.** Prints **every** `BROOD_*` the runtime reads, from `crates/lisp/src/debug_flags.rs`, grouped with the triage groups first (attribution / JIT / optimizer levers / GC / scheduler / engine) and the rest after (diagnostics-and-checking / host environment), with a dependency's flags marked `[not brood's]`. Exists because this table is the only place the flags were documented and it does not ship in the binary. **Both directions are gated by tests** (2026-09-04): every catalogued name must still exist in the source, so a rename cannot leave a line telling you to set something the runtime ignores; and every `BROOD_*` read under `crates/*/src` or `std/` must be catalogued, so the gap cannot re-open. It had re-opened to 43 of 101 — the worker count, the reduction budget and every GC tuning knob were undocumented here on the theory that the printed list should stay a *performance* subset, which is exactly what they are. Build-time names (`BROOD_GIT_SHA`, `BROOD_STDLIB_HASH`, `BROOD_EMBED_RUNTIME`) are exempted by an explicit allow-list, not by omission. |
| `BROOD_PERF_STATS=1` | Dump the VM work-attribution counters (`(vm-stats)`) to stderr after a file/`--test` run — closure activations, IC hit/miss, prim inline/fallback, env-chain hops, allocs, defers — **plus the `ns_*` TIMING accumulators** (`perf_time!`: spawn, deliver, message copy in/out, receive, matcher resolve, teardown, and one scheduler quantum, which nests the rest). **Needs `--features perf-stats`** (else prints a hint; both compile to nothing by default). The counts answer "how much work"; the `ns_*` shares answer "**where**" — read shares of `ns_quantum`, not a sum, and confirm any winner with a counter-free A/B, since the atomics perturb timing. See `docs/benchmarking.md`. |
| `BROOD_JIT_DUMP_IR=1` | Dump each fully-lowered JIT arm's **bytecode opcode fingerprint + Cranelift CLIF** to stderr (`[jit-ir]` lines), for diagnosing a JIT miscompile — read the IR, diff against the intended semantics. **Needs `--features jit`**; only fires for arms that lower (a bailed arm never reaches the dump — pair with `BROOD_JIT_BAIL_TRACE` for the refusals). Run a *targeted* program to limit which arms compile. Since 2026-08-11 the **scalar-register worker reports too** (`scalar-register: i64\|f64` in place of `ckpt_slot:`, no CLIF — its IR is built and finalized in one pass): it previously emitted nothing, so `fib`/`pfib` — the arms it wins biggest on — read as never-lowered here, and an arm that stopped taking that path was invisible to every gate but a benchmark. |
| `BROOD_JIT_BAIL_TRACE=1` | Name each arm a lowering **refuses or demotes**, with the reason (`[jit-bail] arm=<name> reason=…` — the profitability gate's `call-mediated-boxed` (`jit_plan::plan_general_lowering`), pre-checks like `chunk-outside-jit-subset`, mid-emit refusals like `call-spill-exhausted`, and runtime demotions like `deopt-thrash-latched` and `suspend-latched`). The complement of `BROOD_JIT_DUMP_IR`: that tool shows what lowered, so a refusal was only ever visible as *absence* — indistinguishable from an arm that was never hot or never tried. Reach for it when a row is slower than expected and you want to know whether the hot arm was rejected on purpose. **Needs `--features jit`**; one cached `var_os` when off. |
| `BROOD_NO_INLINE=1` | **Opt-OUT** of the JIT recursive self-inliner (Phase B, `docs/jit-optimizing-tier.md` §6b) — now **default ON** via two-stage tiering (devlog 2026-06-17: dual-body + per-engine frame sizing + a deferred lower-priority inlined upgrade, so the VM keeps the small body and short-lived workloads stay on the small native — fib ~1.7×, spawn/bintree/nqueens flat). Set it to fall back to the small-native-only baseline (the A/B lever). **Needs `--features jit`**; `BROOD_INLINE_DBG=1` traces which arms qualify to inline. |
| `BROOD_NO_PARTIAL_LEAF=1` | **Opt-OUT** of **partial** leaf splicing (ADR-210) — default ON since 2026-08-03. A derivation may keep a **residual non-tail call** beside the spliced leaves, because the leaf-spliced layout now carries its own deopt checkpoint and a deopt resumes in the *spliced* chunk (`ir::LeafInline::resume`). Before this, one un-spliceable callee blocked inlining of every small callee beside it — `mandelbrot`'s `->float` next to the recursive `esc`. Set it to revert to all-or-nothing splicing: the A/B lever, the bisect lever, and the stopgap if a duplicated effect is ever suspected — which is why it has a switch at all, since that failure mode is a silently *repeated* effect, not a crash. Guarded by `tests/jit_effect_once_test.blsp` cases 5–6 (verified by sabotage: no journal → the loop case counts 50 179 of 50 000). **Needs `--features jit`**. |
| `BROOD_NO_LEAF_INLINE=1` | **Opt-OUT** of the JIT leaf-callee inliner (Phase 2, `docs/jit-optimizing-tier.md`) — **default ON since 2026-07-19** (boot/`require`/`nest check`/suite/benchmark rows measured flat; scalar-helper loops ~30%, type-predicate dispatch a further ~8% on top of the `type-of` prim). Set it to disable the splice (the A/B / bisect lever, the leaf sibling of `BROOD_NO_INLINE`). **Needs `--features jit`**; `BROOD_INLINE_DBG=1` traces leaf derivations too. |
| `BROOD_NO_FLOAT_GLOBAL=1` | **Opt-OUT** of float-global unboxing — **default ON since 2026-07-29**. The tier-time profile types only an arm's *params*, so an arm whose floats come from a `def`'d constant (nbody's `advance-body (b i)` × the global `dt`) read as non-float-context, lowered `(* dt x)` onto the integer path, and deopted on **every** activation until deopt feedback marked it `BAILED` — running interpreted for the whole program (`nbody` 1.8×). The fix records which read globals held a `Value::Float` at tier time and unboxes those reads behind `as_f64`'s tag guard (a stale guess deopts, never miscompiles). Set it to A/B or bisect. **Needs `--features jit`**. |
| `BROOD_TIER=0\|1\|2` | **The tier ceiling** (ADR-222) — how high execution may climb the ladder: `0` tree-walk, `1` bytecode VM, `2` native. One knob replacing two unrelated env reads; `BROOD_VM=0` and `BROOD_NO_JIT=1` are kept as **aliases** for ceilings 0 and 1. `BROOD_TIER` wins if both are set, and an unrecognised value falls through to the aliases rather than defaulting, so a typo cannot silently lower the ceiling. Measured ladder, same binary and program (`collatz`, `BENCH_N=300000`): **0.12 s / 3.24 s / 57.37 s** for ceilings 2/1/0. |
| `BROOD_NO_JIT=1` | **Runtime JIT off-switch:** `jit_tier` never compiles or runs native code — every arm interprets on the (correct) VM. The A/B / correctness lever for ruling a JIT-only miscompile in or out (and a stopgap around one) **without a no-`jit`-feature rebuild**. **Needs `--features jit`** (a no-jit build has no JIT anyway). Confirm via `BROOD_JIT_DUMP_IR=1`: 0 `[jit-ir]` lines with it set. |
| `BROOD_JIT_VERIFY=1` | **Runtime JIT self-check (works in plain `--release`, no debug-assertions):** scans every Brood→Brood call's staged args for a stale LOCAL handle (use-after-GC) and prints `[jit-verify] STALE <kind> … for call to '<fn>'` at the staging site. The release-capable counterpart of the debug-only `[jit-staged-stale]` check — for catching a JIT+GC miscompile (bug #2) in a normal binary. **Needs `--features jit`**; off by default (one cached bool + a short per-call scan when on). |
| `BROOD_JIT_VERIFY_FN=<fn>` | **Targeted value-level trace (any build):** logs every JIT'd Brood→Brood call to `<fn>` with each staged arg's type — `[jit-verify-fn] call to '<fn>' arg[k] = NIL\|int\|float\|map\|…`. Pinpoints a *value-level* corruption the handle scan can't see (a `nil`/wrong value staged where a number belongs — e.g. pong's `badge-ops` getting `throb=nil`): shows whether the bad value is staged *from JIT'd code* and which arg position. **Needs `--features jit`**. |
| `BROOD_DEOPT_TRACE=1` | Print each JIT type-deopt's arm name + `deopt_watch` flag to stderr (`[deopt] arm=advance-body watch=true`) — for finding which arm keeps falling off the native path (the deopt-feedback / matmul-class signal). **Needs `--features perf-stats`.** |
| `BROOD_VM_TRACE=1` | Trace each bytecode instruction to stderr as it executes (`[vm-trace ip=N] InstName(...)`). Debug builds only. For debugging VM/JIT correctness divergences. |
| `BROOD_GC_TRACE=1` | Log each minor GC collection's nursery/old-gen stats to stderr (`[gc-trace] collect: ...`). Debug builds only. |
| `BROOD_DEFER_DBG=1` | Name each closure call that **defers to the tree-walker** (`[tw-defer] <name> argc=N`, the `dispatch.rs` fallback for a closure with no VM-eligible arm). The `tw_defer` counter says how MANY; this says WHO — and one deferred entry tree-walks everything below it transitively, since the tree-walker's `apply_closure` never re-enters the VM. This is what found §7.3's expansion constant (a quasiquote invisible to the static rewrite inside a vector literal deferred `%receive-split`, and with it the whole match-lowering machinery). Works in release; one cached `var_os` when off. |
| `BROOD_EVAL_TRACE=1` | Trace each form entering the tree-walking evaluator to stderr (`[eval-trace] <form>`). Debug builds only. Use to see which forms the VM defers to the tree-walker. |
| `BROOD_BOOT_TRACE=1` | Print the phase breakdown of the shared prelude build to stderr — the cold source boot (`[boot] builtins=… read=… expand=… eval=… freeze=…`, plus a `[boot-form]` line for any form whose expansion takes >300µs) **and, since 2026-08-26, the warm cache-hit path** (`[boot] parse=… eval=… freeze=…`), which previously reported only a total and so could not say whether a boot regression was in reading the cache, evaluating the prelude, or one `require` inside it. Works in release. This is the tool that found KI-61's two halves: 12.1 ms of the 26 ms warm boot was two `require-one` forms, 3.5 ms a second positioned read of the prelude (both now gone — ADR-246/247, warm boot 11.6 ms). |
| `BROOD_JIT_CB_TRACE=1` | Trace JIT runtime-callback invocations to stderr (`[jit-cb] brood_rt_<name>(...)`). Debug builds only. Useful for diagnosing JIT-compiled code calling back into Rust (global lookup, slow calls, GC). |
| `BROOD_NO_DROP_WARN=1` | **Opt-OUT** of the once-per-name warning that a message was **dropped for a registered name no process holds** (ADR-232) — default ON. The drop itself is unchanged Erlang semantics (`send` is fire-and-forget; a missing *name* is silent, only a missing *node* raises under `:send-errors`); what this prints is the fact that it happened, at the receiving node, which is the only party that knows. Default-on because a flag you must arm before the bug is a flag that is absent when it matters — see KI-39, whose retroactive self-reporting also failed to fire. Deduplicated per name, so a hot loop addressing a dead service warns once rather than flooding. This is the fix for **KI-36**'s root diagnosability gap, which cost twelve days and three sightings. |
| `BROOD_NO_CRASH_REPORT=1` | **Opt-OUT** of the **default crash reporter** (ADR-305, `std/proc/crash-report.blsp`) — default ON in `brood file`, `nest run`, a released bundle and the REPL (never under `nest test`). A `proc/system-monitor` subscriber selecting `:exit-abnormal` prints one report per crash *site* — pid, reason, kind/code/position, hint, up to eight trace frames — for any process that exits with a reason other than `:normal`/`:kill`/`:shutdown`; the kernel's own `process N died: …` one-liner (no trace, repeated per iteration of a crash loop) stands down while one is armed. With this set the one-liner is back. Not a debug flag so much as the switch for a test harness or a host with its own reporting: pass `{:sink f}` to `crash-report/start` to capture reports as strings instead. |
| `BROOD_NO_RELOAD_DIAG=1` | Silence the hot-reload `def` diagnostics (`[reload] arity changed …`, `[reload] macro … redefined`). For a tool that rebinds globals *deliberately and en masse* — `nest test --cover` wraps every project function in a variadic shim, so it sets this itself. Off-switch only; the default stays on so an accidental reload mismatch is still surfaced. In-language equivalent: bind the global `*reload-diagnostics*` to false (the kernel checks both), which is what a test exercising a deliberate arity change does. |
| `BROOD_DUMP_CODE=<substr>` | Dump the **native disassembly** of each JIT'd arm whose `defn` name contains `<substr>` — the machine-code counterpart of `BROOD_JIT_DUMP_IR` (which stops at CLIF), for when a miscompile has to be read at the instruction level and gdb can't see the anonymous JIT pages. **Needs `--features jit`**; substring-filtered so one targeted arm doesn't bury you. |
| `BROOD_NO_TW_REENTRY=1` | **Opt-OUT** of the tree-walker→VM router — **default ON since 2026-09-04 (ADR-318)**: a VM-eligible closure invoked from TREE-WALKED code runs through `vm_apply` instead of inheriting its caller's ~10× engine (before it, `apply_closure` never re-entered the VM, so ONE deferred call tree-walked everything beneath it — §7.3's expansion-constant mechanism). Bounded by `TW_REENTRY_BUDGET` (32) so mixed-eligibility mutual TAIL recursion stays flat (`tests/tw_reentry_test.blsp`). Measured: 60× on the viral shape, `startup` −6.9%, standard rows neutral. It shipped opt-in (2026-08-30) because routing exposed **KI-88** — one spawn of a warm 50-burst created+promoted but never scheduled — which was never root-caused and went dormant: 358 clean runs of the repro with the router live, and a default-on stranded-work watchdog that names the pid if it recurs. Set this to A/B, to bisect, or as the stopgap if a routed shape is implicated; if the watchdog ever prints, keep the binary and the log (KI-88). |
| `BROOD_LINMAP=0` | **Opt-OUT** of the linear-map rewrite (the `inline.rs` pass + its `macros.rs` lowering that turns a provably-linear map accumulator into an in-place build). On by default; set it to `0` to A/B the pass or bisect a suspected map-build miscompile. |
| `BROOD_NO_JIT_COMPUTED=1` | Debug bisect: bail (run on the VM) any arm whose chunk contains a computed jump, keeping the rest of the JIT live. Narrower than `BROOD_NO_JIT=1` — use it to test whether a miscompile lives specifically in computed-jump lowering. **Needs `--features jit`**. |
| `BROOD_NO_XCALL=1` | **Opt-OUT** of the **hot re-lowering / inline fast-frame call path** (§7.5 of `docs/compute-frontier.md`) — **default ON since 2026-08-30**. Once an arm's small native is installed and it activates again, the SAME body is recompiled on the deferred queue with the Brood→Brood call ceremony (`jit_run_fast_link`'s field save/restores, frame window, indirect call) emitted **inline in CLIF**, and swapped in by plain pointer (same chunk/frame/checkpoint — none of the two-stage frame-sizing dance applies). Short runs never pay the fatter compile (deferred queue drains only after every initial tier); long-running arms get the win: `bintree` **−10.6%** at default N, **−19%** at 2× run length, every other row noise. Set to A/B, bisect, or as the stopgap if the inline path is ever implicated. The emission stands aside on its own under debug-assertions, `BROOD_JIT_VERIFY`/`_FN` and `perf-stats` builds (the callback path carries those diagnostics). |
| `BROOD_XCALL=1` | Additionally emit the inline call path in **every body's first compile** (the experiment lever behind §7.5's measurements) — rejected as a default: a ~115M-instruction per-run compile CONSTANT (fib +6% at default N; the step-1 `BROOD_MKCLO` cost class) plus `spawn` +19% via contention on the fatter compiles. |
| `BROOD_MONO=1` | **Opt-IN** to ability-dispatch **monomorphization** (ADR-182, Tier 1): the compiler devirtualizes an ability op call whose first arg is a *literal* (`(size 5)`) or a *direct record-constructor call* (`(area (circle 2))`, id proven via the `*record-ids*` registry) to a direct impl call, skipping `identity-of`/`impl-for`. **Off by default** — the trade-off is late binding (a captured impl fn goes stale if that id's impl is re-registered), so default builds keep 100% dynamic semantics. Every uncertainty declines the rewrite (a non-record fn in constructor position is rejected). |
| `BROOD_MONO_DBG=1` | Trace each `BROOD_MONO` devirtualization to stderr (`[mono] devirtualized A/op for :id → direct impl call`). Confirms the rewrite fired (and for which id) — the mono counterpart of `BROOD_INLINE_DBG`. |
| `BROOD_L1_STATS=1` | Report the **L1 local-send fast path**'s hit rate at exit (`[l1] local-send fast path: N hit (P%), … not-parked, … value-declined, … over-budget`) — `over-budget` is ADR-245's copy cap declining a large message, kept apart from `value-declined` (a kind the copier does not handle) because the two call for opposite responses. ADR-178's fast path only fires when the receiver is *parked*, so this answers "did it actually apply?" before you attribute a message-row result to it (measured 100% on `pingpong`/`ring`). |
| `BROOD_L1_BUDGET=<units>` | The **copy-work budget** for message work done **under a mailbox lock** — both the L1 local-send copy and the selective-receive peek-in-place rebuild (ADR-245, KI-56); one heap node is one unit, default 4096, and **`0` means unlimited** — the pre-cap behaviour, for A/B and bisect. The copy is deliberately *not* moved out of the lock (the lock is what makes the parked receiver's heap safe to touch; relocating it breaks `shutdown_runtime_parked`), so a message past the budget declines instead and takes the wire path, whose heavy work already happens outside the lock. An unrelated `%mailbox-size` probe measured **p99 1 106 µs at ~80 KB and p50 5 011 µs at ~1.6 MB** uncapped, against a wire path flat at 4–10 µs across a 500× payload range. A typo in the value keeps the default rather than uncapping. `BROOD_L1_STATS=1` reports `over-budget` separately from `value-declined`. |
| `BROOD_NO_MSGTAG=1` | Deliver L1 fast-path messages without their leading-keyword tag, defeating the selective-receive pre-filter for that route. The A/B lever for what the tag carry is worth; off-switch only. |
| `BROOD_NO_HANDOFF=1` | Disable the scheduler's **direct-handoff** wake policy (`process/scheduler/pool.rs`), falling back to plain enqueue. The A/B lever for attributing a latency/throughput change to handoff, and for ruling it out of a wake-ordering race. |
| `BROOD_SPAWN_SPILL=<n>` | Backlog at which a spawn stops going to the **spawner's own worker** and spills round-robin (`process/scheduler.rs`) — default **1**, so a child stays local only while our queue is empty. Placement used to be always-local, which meant a dispatcher spawning a handler per request piled every one onto its own queue where a slow handler blocked the rest; stealing rebalanced only 12%. Measured on the `latency` row, **p50/p99 medians over 11 runs**: always-local 136/735 µs → **spill 1: 27/256 µs** (5.0× / 2.9×), and the sweep is monotonic in both (spill 8 78/457 · spill 4 62/397 · spill 2 48/289). **p99.9 is not resolvable on this workload** — two 11-run samples of the same binaries disagreed by 3× — so no p99.9 claim is made either way. A huge value restores the old always-local behaviour for an A/B. |
| `BROOD_SPAWN_RR=1` | Force **round-robin** spawn placement outright (ignores the spill threshold). Looks best on `latency` p50 and is *not* the default: it costs `supervisor` **2.6×** (862 → 2223 ms) by scattering every child of a request/reply spawn across workers. Kept as the A/B endpoint that shows why the threshold exists. |
| `MIMALLOC_PURGE_DELAY=0` | Not ours — mimalloc's own env option. The allocator holds freed pages, so RSS on a churny workload sits above the live working set; setting this recovers **17%** on a light workload and **~2.3×** on a heavy-churn one, for ~4% throughput. Live data in those runs was ~59 KB against hundreds of MB of RSS, so **RSS is not a proxy for live bytes on this runtime**. The default is the deliberate "spend memory for speed" choice (devlog 2026-06-15). See `docs/runtime-frontier.md` A8 — and note that entry's warning: measure with a FIXED iteration count, never a fixed duration. |
| `BROOD_NO_RECV_MARK=1` | **Opt-OUT** of the **receive-mark** (ADR-195) — default ON since 2026-07-30. A `receive` whose clauses all pin a `ref` this process minted starts its scan past every message that predates the ref (sound: a message enqueued before the ref existed cannot carry it), making a request/reply receive O(1) in the mailbox backlog instead of O(backlog): 32k backlog costs 4 µs armed, 262 µs with this set. Reach for it as the A/B lever, to bisect, or as the stopgap if a message is ever suspected of being skipped — which is why this one has a switch at all: a wrong skip does not crash, it silently fails to deliver. |
| `BROOD_NO_SHARE_FN=1` | **Opt-OUT** of handing an **already-shared closure** across a local send by handle instead of deep-copying its code (`copy_cross_heap`, the L1 parked-receiver path) — default ON since 2026-07-30. Only fires for a closure that is already a RUNTIME-region value (one capturing **no locals**, e.g. the idiomatic `:start (fn () (spawn-link (worker)))`) and only between processes of the **same** runtime; a capturing closure and every cross-node send still copy. Worth 2.4× on supervised `start-child` and 6 µs vs 54 µs per closure send. Set it to A/B, to bisect, or as the stopgap if a shared handle is ever implicated in a fault. **Deliberately does not promote a local closure to make it shareable** — measured, that grows the append-only RUNTIME region proportionally to closures sent (541 MB at 800k transient sends vs 150 MB); see `docs/runtime-frontier.md` A3 before re-attempting. |
| `BROOD_DBG_CONST=1` | Trace JIT constant-pool decisions (`jit/mod.rs`). For diagnosing a wrong-constant miscompile. **Needs `--features jit`**. |
| `BROOD_GUI_GPU=1` | Select the experimental **OpenGL** render backend at *runtime*, so one installed binary can default to softbuffer and opt into the GPU path per run (build with `--with-gui-gpu`). |
| `BROOD_GUI_HEADLESS=1` | Run the GUI/display layer with no real window — also silences audio, so a windowing/audio test stays safe on a headless CI box. |
| `BROOD_AUDIO=0` | Disable `audio-beep` (also off with no device present, or under `BROOD_GUI_HEADLESS`). |
| `BROOD_CONTRACTS=1` | Arm **runtime type contracts** from `sig` declarations (implemented in `std/prelude/core.blsp`, not Rust — a `sig` wraps the function in a checking shim). The runtime counterpart of `nest check`'s static advice. Turns every plain `(sig …)` into a **rebinding**, so a `sig` above its `defn` fails the module's load — `crates/lisp/tests/sig_placement.rs` gates that tree-wide. |
| `BROOD_NO_STDIMAGE=1` | **Opt-OUT** of the stdlib **startup image** (ADR-256/281) — **default ON since 2026-08-28**. A `require`d std module materialises its bindings from `~/.cache/brood/std-image-<stdlib-id>.bin` instead of re-reading and re-evaluating its source: `json` 6.5 → 1.7 ms, `http` 12.0 → 3.6 ms, and a three-module **script 46.5 → 36.2 ms** — a saving a short-lived run pays on every invocation, where a long-lived one amortises it away. Safe by construction: the key is a **content hash** of every baked-in `.blsp` plus the git sha, so a stale image cannot be read, and with none present `install` returns nil in ~30 µs. The runtime never BUILDS one (~1 s, which would land on the short-lived runs it helps); **`nest` writes it**. Stands aside under `BROOD_COVERAGE` — coverage instruments the compiler, and a materialised module is never compiled. It was default-on once before and reverted the same day (KI-72); what justifies the flip now is not that one fix but **ADR-280's differential**, which loads every module from source and from the image and requires the resulting state to match — the first construction-level gate this feature has had. **The trap to know:** the id moves with every commit and every `std/` edit, so an image silently goes stale and the run you believe is imaged is reading source. That is not only a speed question — the source path is a documented amplifier for KI-89, and deleting the images took a green 5514-test `nest test` to **106 failures in 469 s**. **Since 2026-09-04 four things say so out loud, so stop diagnosing this by experiment:** the suite summary prints `(stdlib image: N sections)` or `none — std/ loaded from SOURCE` with the reason; `make doctor` §4 asks each binary directly; `nest` prints one line when it *rebuilds* the image, because that command itself ran from source; and `(stdimage/status)` now carries **`:installed`** — what THIS process materialised at boot — beside `:state`, which reads the disk NOW. Read `:installed` when you want to know what a run actually did; the two disagree exactly when it matters. Note `scripts/build-std-image.sh` defaults to the **debug** profile: pass `release` when the binary you are about to run is `target/release`, or it writes a perfectly good image for the other binary and reports success (it now names which). |
| `BROOD_IMAGE_TRACE=1` | Name every module actually **materialised from an image** (`[image] json`), and time the boot install. The complement to the section-load line, which reports only that a section was read: this is what distinguishes a module served by the image from one that loaded from source anyway. **Reach for it before believing any image measurement** — a suite that reports "99 sections installed" and 0 `[image]` lines has exercised none of the image path. Works in release. |
| `BROOD_NO_PRELUDE_IMAGE=1` | **Opt-OUT** of the **prelude image** (ADR-314) — **default ON since 2026-09-04**. A warm boot materialises the prelude's ~808 bindings from `~/.cache/brood/prelude-expanded-<id>.img` instead of reading and evaluating the 544 expanded forms in the text cache beside it: release boot **21.6 → 13.5 ms**, `make ab --floor` reads **`startup` −11.1%** (0.0% floor) with `pipeline` −13.5%, `sieve` −7.3%, `reduce` −7.1%, `strings` −6.8%, `errors-deep` −6.7% and **no regression on any of 30 rows**. Set it to fall back to ADR-138's text cache (which itself falls back to the source boot), for an A/B or to bisect a suspected bad artifact; CI's tree-walker job sets it so the text path keeps deliberate coverage. **It shipped default-on twice before and was reverted the same day both times**, each on a fact the evaluation *records* rather than binds: KI-105 (a stale stdlib section directory restored from the image — `%std-image-reinstall!`) and KI-106 (the registry-name set was not carried, so a multi-file `nest check` lost every derived multimethod mirror). Both fixed with sabotage-verified guards; the boot differential compares the registry set; and **`make check-imaged` runs the project's own checker gate with the image on**, because every test written *for* the image passed while KI-106 was live. The cold boot is unaffected by design — it does the full source boot and writes both artifacts. Stands aside under `BROOD_COVERAGE` (a materialised binding is never compiled). The trap to know: this restores **bindings**, so anything the evaluation merely *recorded* — `defdyn` marks, def sites, `meta`, privacy, registry marks — has to be written explicitly; five of those were missed in a row while building it. |
| `BROOD_NO_CHECK_CACHE=1` | Bypass the incremental `nest check` result cache (ADR-129) — recheck everything from scratch. Reach for it when a checker change is in flight and cached results would mask it. Implemented in `std/tool/project.blsp`. |
| `BROOD_TEST_NO_SCOPE=1` | Revert `nest test` from the default **per-file scoped** run (each file `load`ed inside its own `%isolate`) to the legacy load-all-then-run-all path. The escape hatch for a suite that genuinely relies on cross-file top-level `def`s; also the A/B lever for the promoted-code accumulation the scoped path fixed. Presence-checked, so any value enables it. Implemented in `std/tool/project.blsp`. |
| `BROOD_HISTORY=<path>` | Override where the REPL stores its history (`std/tool/repl.blsp`). |
| `RUST_BACKTRACE` | `brood`/`nest` **default it to `1`** (set in each `main`); `RUST_BACKTRACE=0` opts out, `full` for verbose. |

**Two layers of use-after-GC detection** (a moving collector relocates LOCAL
handles; a handle held across a collection without re-rooting goes stale):

1. **Per-deref tripwire** (always on under debug-assertions). Every LOCAL handle
   accessor (`pair`, `env_frame`, `closure`, `vector`, `map`, `string`) in
   `crates/lisp/src/core/heap.rs` checks a poison bit + a 30-bit generation epoch
   (`check_epoch`), panicking at the *instant of the bad deref*. Catches a stale
   handle that's **dereferenced**.
2. **Heap verifier** (`BROOD_GC_VERIFY=1`, `Heap::verify_local_graph`). The
   tripwire misses a stale handle that's **stored** into a heap cell without being
   deref'd (it surfaces far away — an OOB slab index in release, or `promote`
   recursing a corrupted env/closure graph to a `SIGSEGV`). The verifier walks the
   live graph each safepoint and flags it *at the store site's next collection*,
   with the path to the offending cell — so you find the missed-rooting site, not
   the distant blow-up. Reach for this when GC_STRESS gives a `SIGSEGV` or a raw
   index panic rather than a clean tripwire message.

**Crash artifacts.** `brood`/`nest` install a panic hook
(`cli_support::install_crash_dump`) that appends the panic + backtrace to
**`.brood_crash_dump`** in the cwd (in addition to stderr) — durable when a TUI /
`nest run` animation scrolls the message away. Catches Rust *panics*, **not**
`SIGSEGV` (e.g. a use-after-GC blow-up leaves no panic — use `gdb --batch -ex run
-ex bt <test-binary>` for those; `rr` isn't installed, and `valgrind` won't see a
*logical* use-after-GC over safe `Vec` slabs). The first reliable repro of the
scheduler race lives in `docs/claude-demo-findings.md`.

## Working in this repo (the tree changes under you)

**Multiple changes happening at once is normal here.** The user edits files in
parallel — and sometimes renames or commits things mid-task — so the working tree
can change between your reads, and files you didn't touch may differ from what you
expect. Re-read before editing, and treat a moved/changed file as the new reality,
not an error to undo.

**Never run history- or state-altering git commands unless explicitly asked.**
No `git reset`, `git stash`, `git checkout`/`switch` to another branch,
`git restore`, `git rebase`, `git clean`, or force-push — any of these can silently
discard the user's concurrent work. Commit and push only when asked, and commit
the state as it is; don't "tidy" by reverting. If the tree looks inconsistent,
surface it and ask — don't reset to "fix" it.

**Work on `main`. Do not create a feature branch.** Commit straight to `main` and push
there — that is this repo's workflow, and it overrides any default rule about branching
before committing on the default branch. If `origin/main` has moved ahead (it often has;
see the parallel-edits note above), fast-forward local `main` to it first
(`git merge --ff-only origin/main`) and put the work on top; **re-run the suite on the
combined tree before pushing**, because your green run was against the older base and the
combination is untested until you test it.

**Do not add a `Co-Authored-By: Claude` trailer (or any Claude/AI co-author
attribution) to commits in this repo.** Write commit messages with no AI
co-author trailer, overriding any default that would append one.

## Conventions & invariants (don't break these)

- **Proper tail calls are load-bearing.** `eval` is a `'tail: loop`. When adding
  a special form that has a body or branches, evaluate all-but-last for effect
  and hand the *last* form back to the loop (`expr = …; continue 'tail;`) — see
  the `tail_of`/`tail_of_vec` helpers. Don't turn tail positions into plain
  recursion; the test `tail_calls_do_not_overflow` (sum to 100,000) guards
  this.
- **All heap construction goes through `value.rs` helpers** (`cons`, `list`,
  `sym`, `str_val`, …). This keeps the planned `Rc` → `gc-arena` migration
  contained (ADR-002). Don't scatter `Rc::new(...)` of `Value`s elsewhere.
- **Prefer Brood over Rust** — see the "write the language in the language"
  principle above (ADR-006). If something can live in `std/` instead of
  `builtins.rs`, put it there. Add a Rust builtin only when it genuinely needs
  Rust.
- **Favor the simplest user-facing design; defer power features** (ADR-011).
  When a feature has a simple form and a powerful-but-complex form, ship the
  simple one and defer the rest until a concrete need justifies it. Additive
  features cost nothing to delay; every knob is a tax on every user, forever.
- **Keep the language as small as possible.** Minimize the *core* — special
  forms and evaluator semantics — above all. When a feature can be a macro over
  a primitive function instead of a new special form, do that (e.g. `try`/`catch`
  is a macro over a `%try` primitive, not a special form). Primitives are just
  Rust functions; special forms are language. Prefer adding the former.
- **Symbols are interned `u32`s.** Compare with `==`; get the spelling via
  `value::symbol_name`.
- **Truthiness:** only `nil` and `false` are falsy (`eval::truthy`).
- **Brood data is immutable. This is absolute — do not weaken it** (ADR-026;
  `docs/language.md` §Immutability). Every `Value` is immutable: there are **no
  data-mutation primitives** (no `set-car!`, `vector-set!`, `string-set!`, no
  atoms/cells/refs, **no `transient`/`assoc!`/`persistent!`**) and **none may
  ever be added**. Every builtin returns a *fresh* `Value` rather than mutating
  one. **Do NOT add a "sneaky" mutable structure** — not a transient, not a
  builder cell, not a mutable buffer, not an identity-mutable anything exposed to
  the language — no matter how much it would speed up a build. We tried a
  user-facing transient once; it was removed precisely because it violated this
  rule. If you need fast bulk construction, do it as a **GC-quiet in-place build
  *inside one Rust builtin*** that still returns a fresh immutable `Value` (e.g.
  `%map-into` / `map_from_pairs`'s watermarked CHAMP build) — that is an
  implementation detail of *constructing* the value, never a mutable `Value` the
  language can observe.
  - **The ONE and ONLY mutable structure is `Value::Table`** (Brood's ETS): a
    shared, identity-mutable key→value store behind an opaque handle, which
    deep-clones keys/values in and out so no two processes ever alias stored
    data. Everything else is immutable. Reach for a `Table` (or a **process**
    holding state in its loop) when you genuinely need mutable state — never a
    mutable data value.
  - `let`/`fn` bindings never change after creation. The only *binding* mutation
    (not data mutation) is `def` rebinding a *global* — load-bearing for
    Erlang-style hot reload (ADR-013). There is **no `set!` and no `while`**:
    loops are recursion (proper tail calls give O(1) stack) or processes.
  - **Assume immutability everywhere and keep the code simpler for it.** Because
    data never mutates, the kernel needs **no write barriers for data** — the
    tracing GC's minor flip relies on the invariant that *old never points to
    young* for every value (the sole remembered-set is for `def`/env-frame
    *binding* rebinding, ADR-013), values are safe to share/freeze/send, and the
    append-only shared `RUNTIME` region stays sound. Don't add machinery
    (barriers, epoch re-anchors, defensive copies) that only a mutable structure
    would require — there are none to support.
- **Types are set-theoretic, gradual, and advisory** (ADR-023/024;
  `docs/types.md`). A type *is* a set of runtime `Tag`s; subtyping is set
  inclusion; redefinable globals are `dynamic()`, never `Any`. The checker
  **never gates the live image and never warns on a use valid for the image's
  current state** — a `def`/reload always wins, and the checker re-derives on
  every reload (ADR-123/124/125/126; `docs/type-soundness-reload.md`). It still
  *warns* advisorily, and the one hard reject is **batch/CI only** (`nest check`
  exits nonzero on any warning). This supersedes the older "checking never
  rejects a runnable program" phrasing. Before adding a `Value` kind, primitive, special
  form, or pattern, check it against the **compatibility contract** in
  `docs/types.md` — several points are compiler-enforced (a new `Value` needs a
  `Tag` + bit in `types.rs`; primitives will need a signature like `Arity`). Not
  the TypeScript route.
- **Runtime crates are allowed when they remove real complexity.** Prefer our
  own substrate, but a well-scoped crate that genuinely cuts complexity (or
  hand-rolled `unsafe`) is fine in the `brood` lib crate — e.g. `boxcar` backs
  the shared RUNTIME code region (lock-free append-only, stable refs). The bar
  is *infrastructure that helps build the runtime*, not Lisp-callable behaviour:
  functions the language exposes should still be written in Brood (`std/`), not
  pulled from a crate. Dev/UX deps in the **CLI** crate (e.g. `rustyline`) are
  fine. (Relaxes the earlier dependency-free rule / ADR-005.)
- **A runtime's inner processes share live code; separate runtimes don't.** A
  runtime has one shared, mutable code region + global table (`RuntimeCode`,
  behind `Arc`); all processes it `spawn`s share that same `Arc`. So a `def`
  (which `promote`s code into the shared RUNTIME region, then rebinds in the
  shared table) is visible to a *running* spawned process on its next lookup —
  late binding gives Erlang-style hot reload across processes, no restart. The
  prelude stays a separate, immutable, shared-read-only region. **Separate
  runtimes (future nodes) stay independent** — each has its own `RuntimeCode`,
  so updating one never propagates to another. Data is *not* shared: each
  process has its own LOCAL data heap; messages cross as deep copies.
  (See `docs/shared-code.md`; supersedes the earlier "instances are independent
  / no shared mutable global" decision.)

## When you add a feature

1. Implement it (special form in `eval/mod.rs`, or builtin in `builtins.rs`, or
   prelude fn in `std/prelude/*.blsp` — nine bare-root files concatenated in the order
   `lib.rs` lists, NOT the single `std/prelude.blsp` that older docs name).
2. Add tests — an `(assert= …)`/`(is …)` inside a `(test …)` within a `describe`
   block in a `tests/*_test.blsp` file (in-language, via the `std/tool/test.blsp`
   framework: open the file with `(defmodule foo-test (:use test) (:use foo))`
   so the test macros and the module under test refer bare — the header is how you get them bare —
   a qualified reference like `test/run` merely LOADS the module and leaves
   `describe`/`test`/`assert=` qualified; there is no bound `require`, and
   `require-one` is the function underneath), and/or a Rust case in `crates/lisp/tests/`.
   **Every language feature must also be tested across multiple cores**, not just
   single-threaded. The in-language suite already helps here — `std/tool/test.blsp`
   runs each test in its own green process on the ≈`nproc` worker pool, so a plain
   `describe`/`test` exercises the feature concurrently with every other test. On
   top of that, add **explicit concurrency coverage** for any feature that
   produces or carries values: `spawn` workers that build the value, `send` it
   between processes (which deep-copies across per-process heaps — so it proves
   `to_message`/`from_message` *and* `promote`/freeze round-trip the value), read
   it from a shared global in many processes at once, and fan-in the results.
   See the `:isolated` "across processes" block in `tests/maps_test.blsp` for the
   pattern. **Caveat:** a `test` body runs in a green process with bounded
   recursion depth, so prefer **tail-recursive** loops (O(1) stack). Deep *non*-tail
   recursion is no longer an uncatchable segfault: under the VM it hits the
   `MAX_BC_FRAMES` (~1M) frame cap and raises a clean, catchable `recursion too
   deep` error (the tree-walker has the equivalent byte-budget guard), so a runaway
   test fails its own process and the runtime survives. (Verified 2026-06-29; see
   `docs/devlog.md`. The crash-dump tooling below still targets genuine SIGSEGVs —
   e.g. a use-after-GC blow-up — not this case.)
3. **Sabotage every guard you write — a guard that cannot fail is worse than none,
   because it reads as coverage.** Break the fix, watch the test go red, restore. Three
   guards written in one session on 2026-09-01 passed against the very bug they existed to
   catch, and only the sabotage run exposed them: one asserted a *flag* the fix sets (a
   regression that sets the flag without doing the work passed); one exercised the helper
   the fix added instead of the entry point (`session::open`) the bug actually sat in; one
   was anchored to a loop that runs before the files it greps exist. The pattern in all
   three: **assert on the entry point a caller reaches, not on the piece you just wrote**.
   When a change has no clean inverse to sabotage, find an argument from construction
   instead (writing 1.6 MB to a child that never reads *cannot* return if the write is
   synchronous, since a pipe buffer is 64 KiB) — do not settle for a weaker test.
4. Update `docs/language.md` (it documents the language *as implemented*).
5. Tick it off in `ROADMAP.md`; add a dated entry to `docs/devlog.md`.
6. If it reflects a real design choice, record an ADR in `docs/decisions.md`.

## When you RENAME or REORDER a stdlib function

A rename wave or an argument reorder is not a find-and-replace, and treating it as one cost
ten bugs — six of them silent — during ADR-302/307. The trap is that "the function's name" is
not one string. **Enumerate these before the first edit, not one suite run at a time.**

**A call has five spellings.** A textual rewriter sees only the first:

| spelling | example | notes |
|---|---|---|
| bare | `(map xs f)` | |
| qualified | `(stream/map s f)`, `(string/join xs sep)` | `(join …)` does NOT match `(string/join …)` |
| root-scoped | `(/map xs f)` | the `/name` prelude escape |
| macro-constructed | `(list '/mapv 'args '%identity-of)` | **does not exist until expansion** — ungreppable as a call |
| string-embedded | `%load-string`, `reflect/eval-string`, and `nest new`'s scaffold templates | executable, unlike a highlighter fixture |

That last row needs a human read: a sweep finds ~118 strings containing `(map …)` and only
~15 are executable.

**The two files no gate can see: `stress/fuzz_programs.py` and `stress/scaling_probe.blsp`'s
driver.** The stress tooling writes Brood from Python and shell, so `check-corpora` — which
statically checks `stress/**.blsp` for names that no longer resolve — never saw either.
ADR-302 left the fuzz generator emitting `(map f coll)` for three days: 14 of 25 seeds
reported "diverged" on *checker warnings* rather than engine behaviour, so the differential
arm was noise, and the file's own comment records the SAME thing happening after the
previous wave. The scaling probe was worse: it called `bit-and`/`now`/`println`, printed
nothing, and its gate reported "no timing" to nobody, because nothing runs it outside a
manual `make stress`. Both are now covered — the probe is a real `.blsp` the corpus gate
reads, the generator says `STALE GENERATOR` instead of blaming the checker, and the nightly
runs the suite — but a generator cannot be static-checked, so **grep the Python too**. The scaffold templates matter most — they emit a *user's* project, and
nothing in this repo's suite runs the code they generate.

**The Rust checker encodes argument positions structurally**, and they degrade to `any`
*silently* rather than erroring — `types/check/sigs.rs` (curated signatures + callback
demand in `specialize_call`), `types/check/infer.rs` (result inference for
`map`/`keep`/`interpose`/`reduce`/`fold`), `types/check/walk.rs` (callback-signature
synthesis). Two gates now guard this and **both must stay green**:

- `types::check::sigs::convention_tests::curated_combinators_put_the_callback_last` — asserts
  every curated signature taking a callback puts it LAST. It caught a stale
  `seq/index-where` on its first run.
- `tests/introspection_test.blsp` "combinator result inference survives the argument order" —
  pins a known-*precise* inferred type per combinator, so a stale position in `infer.rs`
  fails by name instead of quietly widening to `any`. Pin a case the checker can actually
  prove; `any` is what a broken position yields too.

**`map` is a function name AND a type name.** `(map K V)` in a `sig` is a *type*, and the
two-token form `(map -> int)` parses as a two-argument call — 18 signatures were silently
swapped this way. Check `(sig …)` **and** `(sig! …)`.

**Prefer the reorder that fails loudly.** Moving a collection into argument one fails at once
(a function where a collection belongs errors), which is what made ~1780 sites tractable.
Reordering a *callback's own* parameters fails silently — `(fn (acc x) (+ acc x))` keeps
working, `(cons x acc)` keeps running and builds garbage. Do the first; refuse the second
(ADR-308 keeps `(fn (acc x))` for exactly this reason).

**`doctest` is the completeness gate.** `tests/doc_examples_test.blsp` and
`tests/doctest_test.blsp` *execute* every indented `form → result` example in every
docstring. They caught nine stale examples nothing else would have. Run them early, not last.

**Two measurement rules, learned the hard way in the same session:**
- **Assert the summary line is PRESENT**, never that failures are absent. `nest test | grep
  "test failed"` returns empty both when the suite passes and when the runner died before
  printing anything — and the pipeline's exit code is `grep`'s, not the runner's.
- **Confirm the binary is fresh before believing any result.** A `cargo build` killed by a
  command timeout leaves the old binary in place; the runtime says `this binary's baked-in
  std/ is OLDER than …` on stderr, which a grep for failures filters away. Two diagnoses were
  made against a stale build before this was noticed.

## Known next steps (see roadmap)

The language core (M1) is complete: macros/quasiquote, in-language
`try`/`catch`, maps (CHAMP trie), the string/math/sequence libraries, pattern
matching, modules, project tooling, **dynamic variables** (`defdyn`/`binding`),
the set-theoretic **type checker** (Steps 0–4 + Step 5 structured types — arrows,
element types, parametric HOF results, ADR-078; intersections + `(map K V)` + `?A`
type variables; **gradual checks** via `GradualTy` — `(def x …)`/return-type/
value-position assignment checking, ADR-110), a per-process tracing **GC**
(ADR-035), the **package manager** (ADR-037), the **self-hosted REPL in Brood**
(ADR-048), **LSP Tier 2** (refs/rename, semantic tokens, cross-file nav), and the
**closure-compiling VM** (now the default engine, ADR-076) are all done. `nest check`
is at **zero warnings** across `std/` + `tests/`: the checker is false-positive-clean,
and the few tests that *deliberately* trip a correct lint (non-tail-recursive JIT
torture fns, a redundant `match` clause under test) opt out with the
`(check-allow :category form…)` directive (a runtime no-op marker the checker reads;
see `docs/type-annotations.md`). **Precise body inference** shipped for the catchable
cases — the int-closed and float-contagion arithmetic rules flow a body's provable
type to the return check (`(+ x 1.5)` declared `int` → warns "yields float"). Only the
*merely-wider* residue remains deferred (a body typed exactly `number` declared `int`,
e.g. `(/ x 2)` which is genuinely int-or-float): pinning it would need occurrence/range
analysis and flagging it would false-positive, so it stays out (ADR-011).

The later milestones are underway (vertical-slice style, ADR-045/046):
**M2 editor data model is done** — the `ropey`-backed `Value::Rope` kernel + the
`std/editor/buffer.blsp` immutable-buffer framework; **M3 display protocol** —
`std/editor/display.blsp` render-op vocabulary + `term-*` primitives + the `nest observe`
process viewer; **M4 server/daemon** — distributed nodes (TCP, location-transparent
`send`, monitors, closure-shipping, HMAC handshake) plus a userland
`std/proc/supervisor.blsp` (kernel-supervised processes were tried and reverted — see
roadmap/ADR-039). **Native WASM interop** shipped too (ADR-071/145): an embedded
`wasmtime` host with WIT-typed marshalling + fuel metering (`crates/lisp/src/wasm.rs`,
`std/wasm.blsp`), plus an in-browser playground built on the wasm32 target
(`crates/playground`). The editor app itself is a separate downstream project, out of
scope for this repo and its roadmap. Still ahead here: server-mode socket serving.
