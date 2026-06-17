# CLAUDE.md — working in the Brood repo

Guidance for Claude Code (and humans) working in this project. For the broader
machine setup (Ubuntu, apt, Rust via rustup, etc.) see the global
`~/.claude/CLAUDE.md`.

## What this project is

Brood is a small dynamic **Lisp implemented in Rust**. Its purpose is to be the
language a modern, Emacs-like, self-editing, remotely-hostable text editor is
written in. Today the repo is the **language core**; the editor, display
protocol, and server come later. Read `docs/` before making non-trivial changes
— especially `docs/architecture.md`, `docs/roadmap.md`, and `docs/decisions.md`.

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
- This applies to upcoming pieces too. The **CLI/REPL, the editor commands,
  keymaps, and UI should ultimately be Brood**, with Rust only hosting the
  thinnest necessary substrate. (The REPL is Rust today as a bootstrap; moving
  it into Brood is a goal — see `docs/roadmap.md`.)
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
               heap.rs (per-process heap + shared regions + env chain), alloc.rs,
               blob.rs (cross-process zero-copy blob heap), map_champ.rs (CHAMP
               map trie), sync.rs
  syntax/      reader.rs (text -> Value), scanner.rs, printer.rs, and the tooling
               CST (atom.rs / cst.rs / scope.rs)
  eval/        mod.rs (evaluator — a `'tail: loop` for tail calls + special forms),
               compile.rs (the closure-compiling VM — the default engine, ADR-076),
               macros.rs (quasiquote, macroexpand, the compile pass + pattern lowering)
  types/       mod.rs (Ty/GradualTy set-theoretic lattice), check.rs + check/
               (advisory checker)
  builtins.rs  functions implemented in Rust (the primitive kernel)
  introspect.rs  doc/arglist/global-names/bound? and friends (ADR-025)
  cli_support.rs file-runner / --test plumbing shared by the binaries
  process.rs + process/   green-process scheduler (mailbox, message, monitor,
               scheduler, timer): spawn/send/receive/monitor
  dist.rs + dist/   distributed nodes (handshake, heartbeat, wire) — ADR-033/034
  net.rs       thin non-blocking TCP socket mechanism (ADR-062); Brood policy is
               the external brood-net package
  bundle.rs    single-binary app bundling (ADR-038); gui.rs the GUI frontend (ADR-046)
  error.rs     LispError / LispResult / source Pos
  lib.rs       the `Interp` entry point; bundles std/prelude.blsp
crates/cli/src/main.rs   the `brood` binary — the language (REPL, file runner, `--test`)
crates/nest/src/         the `nest` binary — project tooling (main.rs + mcp.rs) — ADR-028
crates/lsp/src/main.rs   the `brood-lsp` binary — language server (ADR-025, docs/lsp.md)
std/                     standard library written in Brood, grouped (ADR-085):
                         prelude.blsp + bare core (io, file, set, regex, json,
                         fuzzy, format, task, log); the editor/display framework
                         `std/editor/*` (buffer, display, ui, keymap, face,
                         highlight, lineedit, pane, layers, ansi, serve); `std/proc/hatch`;
                         the toolchain `std/tool/*` — grouped on disk but BARE
                         module names (test, project, package, docs, grammar, mcp,
                         observer, proctree, repl, sexp, reload). The net *library* (`net/*`)
                         and `proc/supervisor` were lifted into the brood-net /
                         brood-supervisor packages (Move 2) — but the Rust socket
                         *mechanism* stays in-tree (`crates/lisp/src/net.rs`, ADR-062);
                         only the Brood policy moved out. The REPL is Brood too
                         (`std/tool/repl.blsp`, ADR-048); the binaries bootstrap
                         into `(repl-run)`.
docs/                    architecture, language, roadmap, decisions, devlog
```

The CLI is split (ADR-028, the `rustc`/`cargo` model): **`brood` runs the
language**, **`nest` runs the project**. Both embed the `brood` lib (no
subprocess); `nest` is a thin shell over `std/tool/project.blsp`. `nest` subcommands
today: `new`, `test`, `check`, `run` (with `--watch`), `doc`, `format`, `repl`,
`mcp` (an MCP server over the project), `observe` (the M3 process viewer),
`attach` (the `emacsclient`-style thin frontend for a daemon serving a `ui-run`
app — ADR-090), `grammar` (emit an editor syntax grammar — VS Code TextMate or
Emacs — generated from `(special-forms)`, ADR-092), the package-manager commands
`fetch`/`update`/`tree`/`add`/`remove` (ADR-037), and `release` (single-binary
bundling, ADR-038).

## Commands

```bash
cargo build                       # build the workspace
make test                         # Rust tests + the Brood suite via cargo-nextest
cargo test                        # same, but plain libtest — NO per-test timeout (can hang)
cargo run -p cli                  # start the REPL  (or: ./bin/cli)
cargo run -p cli file.blsp        # run a program file
cargo run -p cli -- --test f.blsp # run one self-contained test file
cargo run -p nest -- test         # discover + run the project's test suite
cargo run -p nest -- new foo      # scaffold a new project
```

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

### Debug tooling — knobs, env flags, and crash artifacts

The kit for chasing GC / use-after-GC and other kernel faults. **Build with
`RUSTFLAGS="-C debug-assertions=on" cargo build --release`** to keep release
speed while arming every debug check below (plain `--release` strips them for
zero shipped cost; plain `cargo build` debug is correct but too slow to expose
contention races).

| Env flag | Effect |
|----------|--------|
| `BROOD_GC_STRESS=1` | Collect at **every** eval safepoint (not just when the threshold is crossed). Turns rare GC races into deterministic ones. |
| `BROOD_GC_VERIFY=1` | **Heap verifier** (debug only): before each collection, walk the whole reachable LOCAL graph and assert every handle is in-bounds + current-epoch. Catches a *stored* stale handle and prints the `root→…→cell` path. See below. |
| `BROOD_TRACE_GCBLOCK=1` | Trace GC-block depth (debug). |
| `BROOD_MEM_LIMIT=<bytes>` | Arm the ADR-043 soft/hard memory cap for a run. |
| `BROOD_STACK_BUDGET=<bytes>` | Raise/lower the non-tail-recursion stack guard. |
| `BROOD_RT_GC_FLOOR=<count>` | Threshold floor (RUNTIME closures) for auto-compacting the shared code region (ADR-091; default 4096). The shared-region counterpart of `BROOD_GC_FLOOR`. |
| `BROOD_PERF_STATS=1` | Dump the VM work-attribution counters (`(vm-stats)`) to stderr after a file/`--test` run — closure activations, IC hit/miss, prim inline/fallback, env-chain hops, allocs, defers. **Needs `--features perf-stats`** (else prints a hint; counters compile to nothing by default). Counting tool, not timing — see `docs/benchmarking.md`. |
| `BROOD_JIT_DUMP_IR=1` | Dump each fully-lowered JIT arm's **bytecode opcode fingerprint + Cranelift CLIF** to stderr (`[jit-ir]` lines), for diagnosing a JIT miscompile — read the IR, diff against the intended semantics. **Needs `--features jit`**; only fires for arms that lower (a bailed arm never reaches the dump). Run a *targeted* program to limit which arms compile. |
| `BROOD_NO_INLINE=1` | Disable the JIT recursive self-inliner (Phase B, `docs/jit-optimizing-tier.md` §6b) so the same binary can be A/B'd with inlining on vs off (mirrors `BROOD_VM=0`/`BROOD_BYTECODE=0`). **Needs `--features jit`**; `BROOD_INLINE_DBG=1` traces which arms inline. |
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
`SIGSEGV` (a coroutine stack overflow leaves no panic — use `gdb --batch -ex run
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
  inclusion; redefinable globals are `dynamic()`, never `Any`; checking never
  rejects a runnable program. Before adding a `Value` kind, primitive, special
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
   prelude fn in `std/prelude.blsp`).
2. Add tests — an `(assert= …)`/`(is …)` inside a `(test …)` within a `describe`
   block in a `tests/*_test.blsp` file (in-language, via the `std/tool/test.blsp`
   framework: open the file with `(defmodule foo-test (:use test) (:use foo))`
   so the test macros and the module under test refer bare — post-ADR-065 a bare
   `(require 'test)` only loads it and leaves `describe`/`test`/`assert=`
   qualified), and/or a Rust case in `crates/lisp/tests/`.
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
   pattern. **Caveat:** a `test` body runs in a green process whose coroutine
   stack is small, so keep recursion in tests **tail-recursive** (O(1) stack) —
   deep *non*-tail recursion overflows it (today that's an uncatchable segfault,
   not a clean error; see `docs/devlog.md`).
3. Update `docs/language.md` (it documents the language *as implemented*).
4. Tick it off in `docs/roadmap.md`; add a dated entry to `docs/devlog.md`.
5. If it reflects a real design choice, record an ADR in `docs/decisions.md`.

## Known next steps (see roadmap)

The language core (M1) is essentially complete: macros/quasiquote, in-language
`try`/`catch`, maps (CHAMP trie), the string/math/sequence libraries, pattern
matching, modules, project tooling, **dynamic variables** (`defdyn`/`binding`),
the set-theoretic **type checker** (Steps 0–4 + Step 5 structured types — arrows,
element types, parametric HOF results, ADR-078; intersections + `(map K V)` + `?A`
type variables; **gradual checks** via `GradualTy` — `(def x …)`/return-type/
value-position assignment checking, ADR-110), a per-process tracing **GC**
(ADR-035), the **package manager** (ADR-037), the **self-hosted REPL in Brood**
(ADR-048), **LSP Tier 2** (refs/rename, semantic tokens, cross-file nav), and the
**closure-compiling VM** (now the default engine, ADR-076) are all done. The checker
is **false-positive-clean** across `std/` + `tests/` (the one remaining `nest check`
warning class is the *intentional* non-tail recursion lint). The remaining
type-system item — **precise body inference** (catching a value *merely wider* than a
declared type, e.g. a `number`-returning body declared `int`) — needs overloaded
arithmetic sigs or occurrence-typing and is the historical false-positive source, so
it stays deferred (ADR-011) until a consumer justifies the risk.

The later milestones are already underway (vertical-slice style, ADR-045/046):
**M2 editor data model** — the `ropey`-backed `Value::Rope` kernel + the
`std/editor/buffer.blsp` immutable-buffer framework are in; **M3 display protocol** —
`std/editor/display.blsp` render-op vocabulary + `term-*` primitives + the `nest observe`
process viewer; **M4 server/daemon** — distributed nodes (TCP, location-transparent
`send`, monitors, closure-shipping, HMAC handshake) plus a userland
`brood-supervisor/src/proc/supervisor.blsp` (kernel-supervised processes were tried and reverted — see
roadmap/ADR-039). Still ahead: the editor app itself, server-mode socket serving,
and the M5 web frontend.
