# Brood

> **Pre-1.0 — on the road to a compatibility promise.** Until 1.0, a 0.x minor release may
> still break things; every break is listed in [`CHANGELOG.md`](CHANGELOG.md), and a renamed
> name's unbound error says where it went. From **1.0.0** the promise in
> [ADR-673](docs/decisions/adr-600-699.md#adr-673--the-10-compatibility-promise-what-is-covered-for-how-long-and-what-is-not)
> holds: semantic versioning, and the language, every public std name, the `project.blsp`
> and lock formats, the `nest`/`brood` command lines and the error codes change only at a
> major release. A deprecated name keeps working, with a warning, until then; nodes of one
> `major.minor` always link. [`docs/road-to-1.0.md`](docs/road-to-1.0.md) is what is left.
>
> Coming from another Lisp? [`docs/brood-among-lisps.md`](docs/brood-among-lisps.md) places
> Brood among Common Lisp, Scheme, Clojure and the rest, and lists what you will look for and
> not find.

**Brood** is a dynamic **Lisp implemented in Rust** with a deliberately small core:
a handful of special forms, with the standard library, REPL and toolchain written in
Brood itself. It runs share-nothing concurrency across every core, connects
distributed nodes, compiles to a bytecode VM with a tier-1 native JIT, and ships an
advisory set-theoretic type checker.

**Live redefinition is the design center.** A running program can rewrite its own
behaviour — redefine a function and every process picks it up on its next call, no
restart.

It is an **immutable** language: data never changes once made and there is no local
mutation (no `set!`, no `while`), so loops are recursion. The one exception is `def`,
which rebinds a global — that *is* live redefinition. Immutability is also what makes
the concurrency safe: a *brood* of cheap processes that share nothing and talk by
messages, with no aliasing across them and nothing to race on.

The command line splits the way `rustc`/`cargo` do (ADR-028): **`brood`** runs the
*language* — a file, the REPL, or a single test file (`brood --test`) — and **`nest`**
runs the *project* — `nest new`, `nest test`, `nest run`, `nest doc`, and dependency
management (`nest add`/`fetch`/`tree`). `make install` puts both, plus the
**`brood-lsp`** language server, on your `PATH`. A brood of processes lives in a nest.

Source files use the **`.blsp`** extension — *Brood Lisp*. Any `.blsp` file means
Brood-language source, as distinct from the Rust kernel under `crates/`.

Also in this repository: the `std/editor/*` framework — an immutable rope/buffer data
model and a serialisable display protocol — for building interactive, self-editing
applications on top.

```lisp
(+ 1 2)                          ;=> 3

(defn square (x) (* x x))        ; params are a LIST (x) — code is lists, data is vectors
(map (list 1 2 3 4) square)      ;=> (1 4 9 16)

(defn greet (name &optional (greeting "hello"))   ; optional arg with a default
  (str greeting ", " name))
(greet "world")                  ;=> "hello, world"

;; recursion is the loop — tail calls use O(1) stack, so this doesn't overflow
(def sum-to
  (fn (n acc) (if (= n 0) acc (sum-to (- n 1) (+ acc n)))))
(sum-to 100000 0)                ;=> 5000050000
```

### Processes & message passing

Under the Lisp is a runtime of cheap, share-nothing **green processes** that talk
only by message passing — the actor model.
`spawn`/`send`/`receive`/`self` are the whole API, and `receive` selects on
**patterns**.

```lisp
;; A worker process: receive a number, reply to `parent` with its square.
(defn square-worker (parent)
  (let (n (receive))
    (send parent (* n n))))

(def me (self))
(def w (spawn (square-worker me)))   ; spawn returns a pid
(send w 6)
(receive)                            ;=> 36

;; Selective receive — match on the shape of the message:
(defn account (balance)
  (receive
    ([:deposit  amt from] (send from :ok) (account (+ balance amt)))
    ([:withdraw amt from] (if (>= balance amt)
                            (do (send from :ok)    (account (- balance amt)))
                            (do (send from :insufficient) (account balance))))
    ([:balance      from] (send from balance) (account balance))))
;; A process loop carries its state in its argument and tail-calls itself —
;; no mutable variable in sight.
```

Distribution is the same model stretched over TCP: two runtimes connect and
`send` works location-transparently across nodes, with remote monitors and
closure-shipping.

## Install

Prebuilt `brood`, `nest` and `brood-lsp` from the latest GitHub release, checked against
its SHA-256 — Linux (x86_64 and aarch64; a static x86_64 build where there is no glibc) and
macOS (arm64 and x86_64) into `~/.local/bin`:

```bash
curl -fsSL https://brood.fly.dev/install.sh | sh
nest update-brood           # later: move to the newest release (--check only reports)
```

Windows (x86_64), in PowerShell, into `%LOCALAPPDATA%\brood\bin`:

```powershell
irm https://brood.fly.dev/install.ps1 | iex
```

How well each is tested (ADR-715): Linux x86_64 runs the full suite; macOS arm64 and
Windows x86_64 run the platform smoke set on every push; the others are built by each
release and not tested beyond that.

To build from source instead: it requires a Rust toolchain (via `rustup`). The build is a Cargo workspace; a
**`Makefile`** wraps the common commands (`make help` lists them all), and an
autotools-style `./configure` records build options.

```bash
# the usual ./configure && make install — installs `brood`, `nest`, and the
# `brood-lsp` language server into ~/.local/bin (a stripped release build)
./configure
make install

make uninstall   # remove the installed binaries
```

`./configure` records build options into `config.mk`; re-run it any time to
change them. Each `--with-X` has a `--without-X` opposite, and a bare
`make install` uses the defaults below (so `./configure` is optional):

| Option | Default | Effect |
|--------|---------|--------|
| `--prefix=DIR`    | `~/.local`  | Install root — binaries go in `DIR/bin`. |
| `--with-jit`      | **on**      | Tier-1 native JIT for hot loops. `--without-jit` falls back to the bytecode VM (for unsupported hosts / minimal builds). |
| `--with-gui`      | off         | Native window backend (winit/softbuffer/cosmic-text) for the display layer. |
| `--with-gui-gpu`  | off         | GPU render target via wgpu: Vulkan, Metal, DX12 (implies `--with-gui`). |
| `--with-audio`    | off         | The `audio/beep` sound output (via rodio); links `libasound.so.2` on Linux, so it's off by default to keep the build portable. |

So the defaults are: **JIT on; GUI, GPU, and audio off; prefix `~/.local`.** For
example, a desktop build with sound:

```bash
./configure --with-gui --with-audio && make install
```

Make sure `~/.local/bin` (or your chosen `PREFIX/bin`) is on your `PATH`.
Run `./configure --help` for the full list.

Building and installing are separate steps. `make release` compiles the
optimized `brood`, `nest`, and `brood-lsp` binaries into `target/release-fast/`
(gitignored) using the `release-fast` profile (stripped, no LTO — so it builds in
a fraction of the time the fat-LTO `release-lean` profile takes, at the cost of a
larger binary); `make install` then just copies those three into `$(PREFIX)/bin`.
Running `make install` on its own builds first (it depends on `release`), so the
one-liner above still works — but you can also `make release` to produce the
binaries without touching the system, and `make install` later to place them.
(`make build` is unrelated: a plain *debug* build of the whole workspace for
hacking on the Rust, which never installs.)

Other handy targets:

```bash
make release   # build the optimized binaries into target/release-fast (no install)
make build     # debug build of the whole workspace (for development; does not install)
make test      # Rust tests + the in-language suite (via cargo-nextest)
make repl      # start the REPL without installing
make benchmark # run the divan benches, archived to docs/benchmarks/
```

Or work straight from Cargo without installing:

```bash
cargo run -p cli                              # start the REPL
cargo run -p cli path/to/program.blsp         # run a program file
cargo run -p cli -- --test path/to/foo_test.blsp   # run one self-contained test file
cargo run -p nest -- new myproj               # scaffold a project
cargo run -p nest -- test                     # discover & run tests/**/*_test.blsp
cargo run -p nest -- run                      # run the project (add --watch to reload)
```

Once installed, the same commands are `brood`, `brood --test …`, and `nest <cmd>`.
The REPL is itself written in Brood (`std/tool/repl.blsp`); `brood` with no
arguments runs it:

```
brood — REPL (Ctrl-D to exit)
brood> (+ 1 2)
3
brood> (defn greet (name) (str "hello, " name))
greet
brood> (greet "world")
"hello, world"
```

### Project commands (`nest`)

`nest` is the project driver (the `cargo` to `brood`'s `rustc`). Run any of these
from a project directory (`nest new <name>` scaffolds one):

| Command | What it does |
|---------|--------------|
| `nest new <name>` | Scaffold a project (`project.blsp`, `src/`, `tests/`, `.mcp.json`, a starter doc). |
| `nest run [file]` | Run the project entry point, or a given `.blsp` file (`--watch` reloads on change). |
| `nest test [files]` | Run the test suite (or specific `tests/**/*_test.blsp` files). |
| `nest check [files]` | Advisory set-theoretic type-check. |
| `nest format` | Reformat every `.blsp` under `src/` and `tests/` in place. |
| `nest doc [module]` | Emit Markdown docs for the project, or one module. |
| `nest repl` | A REPL with every project module pre-loaded. |
| `nest eval EXPR` | Evaluate one expression with the project loaded and print its value: `--in MODULE` inside a module (its private names visible), `--test` as a test sees the world (a scratch home), `-` from stdin. What an assistant uses instead of a throwaway test file. |
| `nest add`/`remove`/`fetch`/`update`/`tree` | Dependency management (ADR-037). |
| `nest publish`/`search` | Publish a version to, and search, the git-backed package registry index (ADR-147). |
| `nest completions [shell]` | Print a shell integration script enabling TAB completion for `nest`. |
| `nest grammar [target]` | Generate an editor syntax grammar (see below). |
| `nest mcp` | Serve the project over MCP on stdio (see below). |
| `nest observe` | A full-screen TUI process observer. |
| `nest dap` | A Debug Adapter Protocol server on TCP: VS Code, Emacs (dape), Neovim and Zed debug a Brood program (ADR-452). |
| `nest profile FILE` | Run FILE under the sampling profiler; write folded stacks for flamegraph.pl / inferno / speedscope. |
| `nest attach` | Attach this terminal to a `ui-run` app served by a running daemon. |
| `nest release` | Bundle the project into a single self-contained executable (ADR-038). |

### Editor & agent integration

**Language server.** `make install` already builds and installs **`brood-lsp`**
(Tiers 0–2: diagnostics, completion, hover, signature help, goto-definition,
references, rename, semantic tokens, formatting). Point your editor's LSP client
at the `brood-lsp` binary for `.blsp` files — see [`docs/lsp.md`](docs/lsp.md).

**Syntax highlighting** is *generated* from the language's own `(reflect/special-forms)`,
so the keyword list never drifts. `nest grammar [target]` prints to stdout —
redirect it into your editor's grammar file:

```bash
nest grammar                 > brood.tmLanguage.json   # VS Code TextMate (default target)
nest grammar emacs           > brood-mode-keywords.el   # Emacs font-lock (brood-mode)
nest grammar tree-sitter     > highlights.scm           # tree-sitter highlight queries
```

**MCP server.** `nest mcp` serves the current project over the Model Context
Protocol on stdio, so an agent (Claude Code, etc.) can eval, look up docs, format,
macroexpand, and run tests against the project's live image. `nest new` scaffolds
a `.mcp.json` wired to it — see [`docs/mcp.md`](docs/mcp.md).

## What works today

Lexically-scoped closures, proper tail calls, `def`/`defn`/`let`/`fn`,
`if`/`when`/`cond`, `and`/`or`, **macros** (`defmacro` +
Clojure-style `` ` ``/`~`/`~@` quasiquote, `reflect/macroexpand`, `reflect/gensym`), integers that
promote to bignums, floats, exact ratios, strings, symbols, keywords, cons-cell
lists, `[ ]` vectors, immutable `{ }` maps (`get`/`assoc`/`dissoc`/`keys`/`vals`/
`contains?`), `#b"…"` **byte strings**, **pattern matching** (`match` +
destructuring in `let`/`fn`, including **bit syntax** — `(bytes
(len :u16) (body len) & rest)`), higher-order functions
(`map`/`seq/filter`/`reduce`/`apply`), and the self-hosting trio
`reflect/eval`/`reflect/read-string`/`reflect/load`. Parameter lists are written as lists (`(x y)` —
code is lists; vectors are data) and support `&optional` (with defaults) and
`& rest`. Code is organised into **modules** (`defmodule`/`:use`/`:alias`) with
enforced privacy — a `defn-` name is module-private. **Dynamic
variables** (`defdyn`/
`binding`) give per-process special vars; an advisory, set-theoretic **type
checker** flags type/arity/unbound-symbol mistakes without ever gating the live
image (see [The type system](#the-type-system) below); and a per-process tracing
**GC** keeps long-running loops flat.
`defn`, the operators (`+`, `<`, …), the sequence library, and the `->`
threading macro are all defined in Brood itself (`std/prelude/`) on top of a
small Rust kernel.

Beyond that: first-class **sets** (`#{…}`), exact **decimals** (`1.50M`) for money,
**`bytes`** with bit syntax, `defrecord` for records (a value kind of its own, with
map-style field access), and **abilities** for open generic functions when a `cond` on
`type-of` can't be extended from outside — each op dispatches on its first
argument's identity (a built-in kind, or a record's own nominal id, so
two record shapes dispatch apart), extensible from any module at any time. Collection
ops are one interface over every kind — `count`/`first`/`conj`/`into`/`get` accept a
list, vector, map, set or `bytes` — and a **keyword is callable** as an accessor, so
`(map people :name)` needs no throwaway lambda. Patterns support alternatives and
conjunctions (`(or 1 2)`,
`(and whole {:keys [a]})`); text has grapheme-cluster-indexed accessors, because a
cursor steps by cluster, not code point; and `seq/transduce` exposes the fusing pipeline
stages so you can write your own.

The names the language ships are **reserved** — `(def get …)` is an error. Your own
code and your packages stay fully redefinable, which is what live redefinition was
always about; the standard library is sticky, and only what you wrote is yours to
patch.

Code runs on a closure-compiling **bytecode VM** (the default engine), and a
tier-1 **JIT** compiles hot compute loops to native code via Cranelift. The one
mutable structure in the whole language is `Table` — a shared, identity-mutable
key→value store for when you genuinely need mutable state; every other value is
immutable, and per-process state lives in a process loop's arguments instead.

See [`docs/language.md`](docs/language.md) for the full reference.

### The type system

**Dynamically typed at runtime, strongly typed in what it refuses, and
set-theoretic + gradual in what it checks statically.** The static layer follows
**semantic subtyping** (Castagna, Duboc, Valim), not TypeScript's
pragmatic-but-unsound one.

*Dynamic.* Every value carries a runtime tag that `type-of` observes
(`:int`, `:string`, `:vector`, `:pid`, `:decimal`, …); nothing is erased, and
dispatch happens on real values.

*Strong — no implicit coercions.* The language would rather raise than guess:

```clojure
(+ 1 "2")          ;=> RAISED +: expected number, got string ("2")
(= [1 2] '(1 2))   ;=> false   ; a vector is not a list
(= 1 1.0)          ;=> false
(/ 3 4)            ;=> 3/4     ; exact, not 0.75
(if 0 :t :f)       ;=> :t      ; only nil and false are falsy
```

`->float` is a function call, not a cast. The one implicit widening is numeric
promotion inside arithmetic (`(+ 1 2.5)` → `3.5`).

*Set-theoretic.* A type **is a set of values**: union, intersection and negation are
the actual set operations, and subtyping is set **inclusion** — semantic, with no
syntactic subtyping rules. The atoms are the runtime tags, so the type language and
the runtime agree by construction.

*Gradual.* What cannot be pinned down is `dynamic()` — a bounded type living *inside*
the same algebra, with consistent subtyping derived from ordinary set inclusion — and
never `Any`. A redefinable global under hot reload is exactly that case, and this is
the valve that lets typing coexist with live redefinition.

*Inferred first; annotations are optional refinements.* The checker infers a signature
for a plain closure, flows element types through `map`/`seq/filter`/`reduce`/`fold`, and
derives body and return types; primitive signatures come from the kernel rather than a
hand-kept table. No annotation is needed to catch this:

```clojure
(reflect/check
  '(defn g (x) (string/upper (+ 1 2))))   ;=> ("string/upper: argument 1 expects string, got 3 ((+ 1 2))")
```

Where you want more, `(sig …)` adds function **arrows** (`(sig area (number -> number))`),
`(map K V)` key/value contracts, intersections, **literal/singleton** types
(`(or :ok 404)`), and `?A` **type variables** resolved per call site.

*Advisory — the checker never gates the live image.* A `def`/reload always wins, and the
checker re-derives on every reload; it warns rather than blocks, and it does not warn on a
use that is valid for the image's current state. The hard reject is **batch/CI only**:
`nest check` exits nonzero on any warning (`std/` and `tests/` sit at zero). If you want
the same discipline at runtime, `nest run` and `nest test` enforce every `sig` as a
contract by default (`BROOD_CONTRACTS=1`; a mismatch blames the caller or the callee) —
the policy is Brood, the kernel only offers each binding to it (ADR-381).

Full detail: [`docs/types.md`](docs/types.md) (the model and its compatibility contract),
[`docs/type-annotations.md`](docs/type-annotations.md) (`sig`), and
[`docs/type-soundness-reload.md`](docs/type-soundness-reload.md) (why checking and hot
reload coexist).

### Performance & benchmarks

Brood runs on a closure-compiling bytecode VM with a Cranelift **tier-1 JIT** for hot
loops, a **generational** per-process GC, and fusing lazy pipelines. Speed is treated
as a measured property, not a claim:

- [**broodlang/brood-benchmarks**](https://github.com/broodlang/brood-benchmarks) — the
  **cross-language suite**: 31 programs across Brood and six other language runtimes (28
  implemented in every language; `spawn-live` runs in five, though only Brood and one other
  process-based runtime provide the same guarantees there — the others are coroutines on a
  shared heap, included so the difference is legible; `supervisor` runs only in those two; and
  `latency` reports **percentiles under a fixed arrival rate**, which is the row that shows
  what a runtime does to ordinary requests while one handler is busy), run under one harness, with the published numbers and the methodology
  behind them — including which rows are like-for-like comparisons and which are not.
  Start there for "how fast is Brood *against other runtimes*"; the docs below are the
  in-repo view of where Brood's own time goes.
- [**docs/benchmarking.md**](docs/benchmarking.md) — how to run and read the
  benches. `make benchmark` runs the [`divan`](https://github.com/nvzqz/divan) suite in
  `crates/lisp/benches/` and **archives each run with full environment metadata** to
  `docs/benchmarks/<UTC-timestamp>.md`, so results are comparable over time rather
  than anecdotal.
- [**docs/benchmarks/**](docs/benchmarks/) — the archive of past runs.
- [**docs/compute-frontier.md**](docs/compute-frontier.md) — where the remaining
  time actually goes, and which levers are open.

Engine selection is explicit, which makes A/B honest: one knob, `BROOD_TIER`, caps
the execution tier — `0` is the tree-walker, `1` the bytecode VM, and `2` (the
default) the VM plus the native JIT. **Build perf binaries with `cargo build --release --bin brood`** — never
`-p brood`, which builds only the library and leaves a stale binary in place (that
mistake once produced a phantom "JIT regression"; see the devlog).

### Relationship to other Lisps — it is *not* a Clojure clone

The surface borrows a few good ideas from Clojure — immutable data, `{ }` map
and `[ ]` vector literals, `:keywords`, `->` threading, and `~`/`~@`
quasiquote — so a Clojure reader will recognise the shapes. But the semantics
diverge, and the differences are deliberate:

- **Concurrency is share-nothing processes + message passing**, not shared memory.
  There are **no atoms, refs, agents, STM, or transients** — no mutable reference
  cell of any kind. State lives in a process, or in a shared `Table`.
- **The loop is recursion with proper tail calls** (Scheme-style). A local,
  self-contained loop is a `letrec`-bound closure called by name; there is no
  `loop`/`recur`, no `while`, and no `set!`.
- **Code is lists, data is vectors.** Parameter lists are written `(x y)`, not
  `[x y]` — the opposite emphasis from Clojure.
- **`def` is late-binding global rebinding** — that *is* live hot reload
  (a running process picks up a redefinition on its next call), not a Clojure var.
  But the language's **own** functions are reserved: no monkey-patching `get` or
  `map`, unlike Clojure's `with-redefs`/`alter-var-root`. Extend with an ability,
  shadow with `let`, or namespace it in a module.
- **A keyword is callable, and nothing else data-like is.** `(:name p)` works;
  `({:a 1} :a)`, `([10 20] 1)` and `(#{1} 1)` are errors with hints — a callable map
  would be a second spelling of `get`, and a callable vector/set answers by
  index-or-membership, an ambiguity Brood refuses.
- **Pattern matching and selective `receive` are first-class**, and it runs on
  its own small Rust runtime, not the JVM.

## Concurrency & distribution

**Processes** (`spawn`/`send`/`receive`/`self`) run share-nothing as lightweight
**green threads** on an M:N worker pool (≈`nproc`), with reduction-counted
preemption, selective `receive` + timeouts, links/monitors and `proc/trap-exit`, and
registered names (see [`examples/processes.blsp`](examples/processes.blsp)).
Supervision is a **userland** `std/proc/supervisor.blsp` over `spawn`/`monitor`
(a kernel-supervisor was tried and reverted). **Distributed nodes** connect over
TCP — two runtimes message each other with location-transparent `send`, remote
monitors, closure-shipping, and an encrypted-by-default HMAC/TLS handshake. On
top of the socket kernel, the in-tree `std/net/*` library gives a bytes-native
TCP/HTTP/SSE stack with TLS (client and server).

## What's next

The **language core** and the **M1–M4 foundation** are complete: everything in
"What works today", plus the concurrency and distribution runtime above, the
**package manager** (`nest add`/`fetch`/`tree`), the **self-hosted REPL**, **LSP
Tier 2** (refs/rename, semantic tokens, cross-file nav), and the
interactive-application stack — a `ropey`-backed **rope kernel** + immutable
**buffer framework** (`std/editor/buffer.blsp`) and a serialisable **display
protocol** (`std/editor/display.blsp`) with terminal and optional GUI frontends,
demoed end-to-end by `nest observe` (a live process viewer) and `nest attach` (a
thin client for a daemon).

What remains is incremental, each item gated on a concrete need (ADR-011): scaling to
very large projects, the performance levers parked in
[`docs/compute-frontier.md`](docs/compute-frontier.md), and the additive language items
ROADMAP lists as still open. The editor application itself is a separate downstream
project, out of scope for this repo.

The full plan is in [`ROADMAP.md`](ROADMAP.md).

## Project layout

```
crates/lisp    the language: reader, evaluator, builtins, value model, scheduler, nodes
crates/cli     the `brood` binary: the language — REPL, file runner, `--test`
crates/nest    the `nest` binary: project tooling — `new`, `test`, `run`, `doc`, `format`, …
crates/lsp     the `brood-lsp` binary: the language server
std/           the prelude + modules: tooling (repl, test, project), net (http/sse/tcp),
               proc (gen/supervisor), editor (buffer/display/ui), … — all in Brood
docs/          architecture, language reference, roadmap, decisions, dev log
```

## Documentation

- [docs/architecture.md](docs/architecture.md) — the design and the "one runtime
  that can also be a server" approach
- [docs/language.md](docs/language.md) — the language reference
- [docs/archive/roadmap-for-v1.md](docs/archive/roadmap-for-v1.md) — the 1.0 language-freeze
  gate (closed 2026-08-29); the list of what Brood permanently *is not* is ADR-713
- [ROADMAP.md](ROADMAP.md) — milestones and status
- [docs/benchmarking.md](docs/benchmarking.md) — how performance is measured;
  archived runs in [docs/benchmarks/](docs/benchmarks/)
- [docs/build-and-ci.md](docs/build-and-ci.md) — why the build profiles and CI jobs are
  shaped as they are, with the measurements behind them
- [docs/backend-seams.md](docs/backend-seams.md) — how swappable the JIT and the
  execution engine actually are, what the backend contract is, and how to read
  performance (`make perf-brood`, `(perf/report)`, `brood --debug-flags`)
- [broodlang/brood-benchmarks](https://github.com/broodlang/brood-benchmarks) — the
  cross-language benchmark suite and its published results
- [docs/protocol-dispatch-design.md](docs/protocol-dispatch-design.md) — how the
  polymorphism seam was designed: the dispatch-identity problem for user-defined
  types, the language survey, and how `ability` resolved it
- [docs/decisions.md](docs/decisions.md) — why the key choices were made (ADRs)
- [docs/devlog.md](docs/devlog.md) — chronological work log
- [docs/handoff.md](docs/handoff.md) — **start here to pick work back up**: current state,
  open threads in priority order, the stress harnesses, and the measurement traps that have
  produced wrong results before
- [docs/brood-for-claude.md](docs/brood-for-claude.md) — the pocket reference for AI
  assistants writing Brood (also embedded in the binary via `%builtin-doc`)

## License

Copyright © 2026 Wilhelm Kirschbaum.

Brood — the interpreter, compiler, and standard library — is licensed under the
**GNU Affero General Public License v3.0** (`AGPL-3.0-only`); see [`LICENSE`](LICENSE).

**Programs you write in Brood are not covered by the AGPL.** Running a program
through the interpreter, and any `.blsp` program you write, may be licensed on
terms of your own choosing — see the additional permission in
[`LICENSE-EXCEPTION.md`](LICENSE-EXCEPTION.md). The copyleft applies to
modifications of the interpreter/standard library themselves.

For a proprietary or otherwise AGPL-incompatible license, contact the author.
