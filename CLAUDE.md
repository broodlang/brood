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

## Layout

```
crates/lisp/src/
  value.rs     Value enum, symbol interner, list/vector constructors  (all heap allocation funnels through here)
  reader.rs    text -> Value
  env.rs       lexical environment chain
  eval.rs      evaluator — a `'tail: loop` for proper tail calls + special forms
  builtins.rs  functions implemented in Rust
  printer.rs   Value -> text
  error.rs     LispError / LispResult
  lib.rs       the `Interp` entry point; bundles std/prelude.blsp
crates/cli/src/main.rs   the `brood` binary — the language (REPL, file runner, `--test`)
crates/nest/src/main.rs  the `nest` binary — project tooling (`new`, `test`, config) — ADR-028
std/prelude.blsp         standard library written in Brood
docs/                    architecture, language, roadmap, decisions, devlog
```

The CLI is split (ADR-028, the `rustc`/`cargo` model): **`brood` runs the
language**, **`nest` runs the project**. Both embed the `brood` lib (no
subprocess); `nest` is a thin shell over `std/project.blsp`.

## Commands

```bash
cargo build                       # build the workspace
cargo test                        # Rust tests + the Brood suite
cargo run -p cli                  # start the REPL  (or: ./bin/cli)
cargo run -p cli file.blsp        # run a program file
cargo run -p cli -- --test f.blsp # run one self-contained test file
cargo run -p nest -- test         # discover + run the project's test suite
cargo run -p nest -- new foo      # scaffold a new project
```

Cargo is the source of truth; a thin **`Makefile`** wraps the common commands as
shortcuts (`make help` lists them): `make build`, `make test`, `make suite`,
`make repl`, and `make benchmark`. The last runs the `divan` benches
(`crates/lisp/benches/`) via `scripts/bench.sh`, which archives each run with full
environment metadata to `docs/benchmarks/<UTC-timestamp>.md`. `make -j$(nproc)`
parallelism isn't relevant — it's a Cargo workspace, not a recursive make.

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
- **Brood is an immutable language** (ADR-026; `docs/language.md` §Immutability).
  Data is immutable — there are **no data-mutation primitives** (no `set-car!`,
  `vector-set!`, `string-set!`, no atoms/cells) and none may be added; every
  builtin returns a fresh `Value` rather than mutating one. `let`/`fn` bindings
  never change after creation. The language's **only** mutation is `def`
  rebinding a *global* — binding mutation, not data mutation, and load-bearing for
  Erlang-style hot reload (ADR-013). There is **no `set!` and no `while`**: loops
  are recursion (proper tail calls give O(1) stack) or, for evolving state,
  processes (`spawn`/`receive`). Genuine mutable state is expressed only two ways
  — a **process** holding state in its loop, or a **Rust-backed opaque resource
  handle** (e.g. the coming rope/buffer) behind primitives — never a mutable
  `Value`. Don't reintroduce mutation to make something convenient; it underpins
  the tracing GC (no write barriers), `Send` per-process heaps + copy-on-send
  messages, and the append-only shared `RUNTIME` region.
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

1. Implement it (special form in `eval.rs`, or builtin in `builtins.rs`, or
   prelude fn in `std/prelude.blsp`).
2. Add tests — an `(assert= …)`/`(is …)` inside a `deftest` in `tests/suite.blsp`
   (in-language, via the `std/test.blsp` framework), and/or a Rust case in
   `crates/lisp/tests/basic.rs`.
3. Update `docs/language.md` (it documents the language *as implemented*).
4. Tick it off in `docs/roadmap.md`; add a dated entry to `docs/devlog.md`.
5. If it reflects a real design choice, record an ADR in `docs/decisions.md`.

## Known next steps (see roadmap)

Macros + quasiquote, dynamic variables (`defdyn`/`binding`), in-language
`try`/`catch`, map literals, and the tracing-GC migration complete the language
core (M1). After that: the editor data model (M2), display protocol + native
frontend (M3), server mode (M4), web frontend (M5).
