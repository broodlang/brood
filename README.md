# Brood

> ⚠️ **Experimental — pre-1.0.** Brood is under active development and nothing is
> stable yet. The language, standard library, tooling, and on-disk formats can
> change dramatically (and break) without notice or migration path. Explore and
> experiment freely, but don't build anything you depend on against it yet.

**Brood** is a small, dynamic **Lisp implemented in Rust**, built for one
purpose: to be the language a modern, Emacs-like text editor is *written in* —
so that a running editor can redefine its own behaviour on the fly.

It is an **immutable** language: data never changes once made and there is no
local mutation (no `set!`, no `while`), so loops are recursion. The single
exception is `def`, which rebinds a global — that *is* live redefinition, the
whole point of an editor that can rewrite itself while running.

Under the Lisp sits share-nothing, message-passing concurrency: a *brood* of
cheap, supervised processes that share nothing and talk by messages. That swarm is where
the name comes from. Immutability is what makes that share-nothing model safe:
no aliasing across processes, messages copied cleanly, no shared mutable state to
race on.

> **Name & tooling.** This project was formerly `mylisp`; it is now **Brood**.
> The command line splits the way `rustc`/`cargo` do
> (ADR-028): **`brood`** runs the *language* — a file, the REPL, or a single
> test file (`brood --test`) — and **`nest`** is the *project tool* —
> `nest new`, `nest test`, `nest run`, `nest doc`, and dependency management
> (`nest add`/`fetch`/`tree`). Both binaries exist today — `make install` puts
> them, plus the `brood-lsp` language server, on your `PATH`. The colony imagery
> is deliberate — a *brood* of processes lives in a *nest*.
>
> Brood source files carry the **`.blsp`** extension — a contraction of *Brood
> Lisp* (`.lisp` collides with Emacs' `lisp-mode`). Any `.blsp` file, or a
> reference to "blsp", means **Brood-language source**, as distinct from the Rust
> kernel.

The editor app itself lives in a separate project (`brood-edit`) and already
exists; it consumes this language and the `std/editor/*` framework. This
repository is the **language core and runtime** — a reader, a closure-compiling
**bytecode VM** with proper tail calls and lexical closures (with a tier-1
**JIT** that compiles hot loops to native code), a Brood-written standard
library, and a self-hosted REPL — plus the **concurrency** and
**distributed-node** runtime, and the editor framework (a rope/buffer data model
and a display protocol) those vertical slices grew into.

```lisp
(+ 1 2)                          ;=> 3

(defn square (x) (* x x))        ; params are a LIST (x) — code is lists, data is vectors
(map square (list 1 2 3 4))      ;=> (1 4 9 16)

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
only by message passing — the actor model Erlang popularised.
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

Requires a Rust toolchain (via `rustup`). The build is a Cargo workspace; a
**`Makefile`** wraps the common commands (`make help` lists them all), and an
autotools-style `./configure` records build options.

```bash
# the usual ./configure && make install — installs `brood`, `nest`, and the
# `brood-lsp` language server into ~/.local/bin (lean, stripped release build)
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
| `--with-gui`      | off         | Native window backend (winit/softbuffer/fontdue) for the display layer. |
| `--with-gui-gpu`  | off         | Experimental OpenGL render backend (implies `--with-gui`). |
| `--with-audio`    | off         | The `audio-beep` builtin (via rodio); links `libasound.so.2` on Linux, so it's off by default to keep the build portable. |

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
| `nest add`/`remove`/`fetch`/`update`/`tree` | Dependency management (ADR-037). |
| `nest grammar [target]` | Generate an editor syntax grammar (see below). |
| `nest mcp` | Serve the project over MCP on stdio (see below). |
| `nest observe` | A full-screen TUI process observer. |
| `nest attach` | Attach this terminal to a `ui-run` app served by a running daemon. |
| `nest release` | Bundle the project into a single self-contained executable (ADR-038). |

### Editor & agent integration

**Language server.** `make install` already builds and installs **`brood-lsp`**
(Tiers 0–2: diagnostics, completion, hover, signature help, goto-definition,
references, rename, semantic tokens, formatting). Point your editor's LSP client
at the `brood-lsp` binary for `.blsp` files — see [`docs/lsp.md`](docs/lsp.md).

**Syntax highlighting** is *generated* from the language's own `(special-forms)`,
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
`if`/`when`/`unless`/`cond`, `and`/`or`, **macros** (`defmacro` +
Clojure-style `` ` ``/`~`/`~@` quasiquote, `macroexpand`, `gensym`), integers &
floats with overflow-checked arithmetic, strings, symbols, keywords, cons-cell
lists, `[ ]` vectors, immutable `{ }` maps (`get`/`assoc`/`dissoc`/`keys`/`vals`/
`contains?`), **pattern matching** (`match` + destructuring in
`let`/`fn`), higher-order functions (`map`/`filter`/`reduce`/`apply`),
and the self-hosting trio `eval`/`read-string`/`load`. Parameter lists are
written as lists (`(x y)` — code is lists; vectors are data) and support
`&optional` (with defaults) and `& rest`. **Dynamic variables** (`defdyn`/
`binding`) give per-process special vars; an advisory, set-theoretic **type
checker** flags type/arity/unbound-symbol mistakes without ever rejecting a
runnable program; and a per-process tracing **GC** keeps long-running loops flat.
`defn`, the operators (`+`, `<`, …), the sequence library, and the `->`/`->>`
threading macros are all defined in Brood itself (`std/prelude.blsp`) on top of a
small Rust kernel.

Code runs on a closure-compiling **bytecode VM** (the default engine), and a
tier-1 **JIT** compiles hot compute loops to native code via Cranelift. The one
mutable structure in the whole language is `Table` — a shared, identity-mutable
key→value store for when you genuinely need mutable state; every other value is
immutable, and per-process state lives in a process loop's arguments instead.

See [`docs/language.md`](docs/language.md) for the full reference.

### Relationship to other Lisps — it is *not* a Clojure clone

The surface borrows a few good ideas from Clojure — immutable data, `{ }` map
and `[ ]` vector literals, `:keywords`, `->`/`->>` threading, and `~`/`~@`
quasiquote — so a Clojure reader will recognise the shapes. But the semantics
diverge, and the differences are deliberate:

- **Concurrency is share-nothing processes + message passing**, not shared memory.
  There are **no atoms, refs, agents, STM, or transients** — no mutable reference
  cell of any kind. State lives in a process, or in a shared `Table`.
- **The loop is recursion with proper tail calls** (Scheme-style). There is no
  `loop`/`recur`, no `while`, and no `set!`.
- **Code is lists, data is vectors.** Parameter lists are written `(x y)`, not
  `[x y]` — the opposite emphasis from Clojure.
- **`def` is late-binding global rebinding** — that *is* live hot reload
  (a running process picks up a redefinition on its next call), not a Clojure var.
- **Pattern matching and selective `receive` are first-class**, and it runs on
  its own small Rust runtime, not the JVM.

## What's next

Concurrency is well underway: **processes** (`spawn`/`send`/`receive`/`self`)
run share-nothing as lightweight **green threads** on an M:N worker pool (≈`nproc`),
with reduction-counted preemption, selective `receive` + timeouts, and process
monitors (see [`examples/processes.blsp`](examples/processes.blsp)). **Distributed
nodes** are in too — two runtimes connect over TCP and message each other with
location-transparent `send`, remote monitors, closure-shipping, and an HMAC
handshake. Supervision is **userland** for now (the `brood-supervisor` package
over `spawn`/`monitor`); a kernel-supervisor was tried and reverted.

The language core is essentially complete — immutable maps, in-language error
handling, pattern matching, modules, the string/math/sequence libraries,
**dynamic variables**, an advisory set-theoretic **type checker**, a per-process
tracing **GC**, the **package manager** (`nest add`/`fetch`/`tree`), the
**self-hosted REPL** (written in Brood), and **LSP Tier 2** (refs/rename,
semantic tokens, cross-file nav) are all done — as is the bytecode VM and the
tier-1 JIT mentioned above.

The editor milestones are well underway as vertical slices: a `ropey`-backed
**rope kernel** + an immutable **buffer framework** (`std/editor/buffer.blsp`); a
serialisable **display protocol** (`std/editor/display.blsp`) with a terminal
frontend (and an optional native GUI window), demoed end-to-end by `nest observe`
(a live process viewer) and `nest attach` (a thin client frontend for a daemon). The **editor app itself is a separate project**,
`brood-edit`, which already exists and consumes this language and the
`std/editor/*` framework. Still ahead here: full server/daemon serving and a **web
frontend**.

The full plan is in [`docs/roadmap.md`](docs/roadmap.md).

## Project layout

```
crates/lisp    the language: reader, evaluator, builtins, value model, scheduler, nodes
crates/cli     the `brood` binary: the language — REPL, file runner, `--test`
crates/nest    the `nest` binary: project tooling — `new`, `test`, `run`, `doc`, `format`, …
crates/lsp     the `brood-lsp` binary: the language server
std/           the prelude + opt-in modules (repl, test, project, buffer, display, …), in Brood
docs/          architecture, language reference, roadmap, decisions, dev log
```

## Documentation

- [docs/architecture.md](docs/architecture.md) — the design and the "one runtime
  that can also be a server" approach
- [docs/language.md](docs/language.md) — the language reference
- [docs/roadmap.md](docs/roadmap.md) — milestones and status
- [docs/decisions.md](docs/decisions.md) — why the key choices were made
- [docs/devlog.md](docs/devlog.md) — chronological work log

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
