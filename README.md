# Brood

**Brood** is a small, dynamic **Lisp implemented in Rust**, built for one
purpose: to be the language a modern, Emacs-like text editor is *written in* —
so that a running editor can redefine its own behaviour on the fly.

It is an **immutable** language: data never changes once made and there is no
local mutation (no `set!`, no `while`), so loops are recursion. The single
exception is `def`, which rebinds a global — that *is* live redefinition, the
whole point of an editor that can rewrite itself while running.

Under the Lisp sits Erlang/OTP-style concurrency: a *brood* of cheap, supervised
processes that share nothing and talk by message passing. That swarm is where
the name comes from. Immutability is what makes that share-nothing model safe:
no aliasing across processes, messages copied cleanly, no shared mutable state to
race on.

> **Name & tooling.** This project was formerly `mylisp`; it is now **Brood**.
> The command line splits the way `rustc`/`cargo` (and `elixir`/`mix`) do
> (ADR-028): **`brood`** runs the *language* — a file, the REPL, or a single
> test file (`brood --test`) — and **`nest`** is the *project tool* —
> `nest new`, `nest test`, `nest doc` (and, later, dependency management). Both
> binaries exist today — `make install` puts them, plus the `brood-lsp` language
> server, on your `PATH`; the Quick start below also shows the raw `cargo`
> equivalents. The colony imagery is deliberate
> — a *brood* of processes lives in a *nest*.
>
> Brood source files carry the **`.blsp`** extension — a contraction of *Brood
> Lisp* (`.lisp` collides with Emacs' `lisp-mode`). Any `.blsp` file, or a
> reference to "blsp", means **Brood-language source**, as distinct from the Rust
> kernel.

The editor itself comes later. This repository today is the **language core**
(v0.1) — a reader, a tree-walking evaluator with proper tail calls and lexical
closures, a Brood-written standard library, and a REPL — plus the Erlang-style
**concurrency** and **distributed-node** runtime, and the first thin vertical
slices of the editor (a rope/buffer data model and a display protocol).

```clojure
(+ 1 2)                          ;=> 3

(defn square (x) (* x x))        ; defn is a macro, written in Brood itself
(map square (list 1 2 3 4))      ;=> (1 4 9 16)

(defn greet (name &optional (greeting "hello"))   ; optional arg with a default
  (str greeting ", " name))
(greet "world")                  ;=> "hello, world"

;; recursion is the loop — tail calls use O(1) stack, so this doesn't overflow
(def sum-to
  (fn (n acc) (if (= n 0) acc (sum-to (- n 1) (+ acc n)))))
(sum-to 100000 0)                ;=> 5000050000
```

## Quick start

Requires a Rust toolchain (via `rustup`).

```bash
# build everything
cargo build

# run the Rust tests + the in-language suite
cargo test

# start the REPL          (installed: `brood`)
cargo run -p cli

# run a program file       (installed: `brood path/to/program.blsp`)
cargo run -p cli path/to/program.blsp

# run a single self-contained test file   (installed: `brood --test …`)
cargo run -p cli -- --test path/to/foo_test.blsp

# project tooling          (installed: `nest <cmd>`)
cargo run -p nest -- new myproj   # scaffold a project
cargo run -p nest -- test         # discover tests/**/*_test.blsp and run them
cargo run -p nest -- doc          # emit Markdown docs for the project
```

`make install` builds and installs both binaries (`brood`, `nest`) into
`~/.local/bin`; `make uninstall` removes them. In the REPL:

```
brood v0.1 — arrow keys to edit, up/down for history, Ctrl-D to exit
brood> (+ 1 2)
3
brood> (defn greet (name) (str "hello, " name))
greet
brood> (greet "world")
"hello, world"
```

## What works today

Lexically-scoped closures, proper tail calls, `def`/`defn`/`let`/`fn`,
`if`/`when`/`unless`/`cond`, `and`/`or`, **macros** (`defmacro` +
Clojure-style `` ` ``/`~`/`~@` quasiquote, `macroexpand`, `gensym`), integers &
floats with overflow-checked arithmetic, strings, symbols, keywords, cons-cell
lists, `[ ]` vectors, immutable `{ }` maps (`get`/`assoc`/`dissoc`/`keys`/`vals`/
`contains?`), Erlang-style **pattern matching** (`match` + destructuring in
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

See [`docs/language.md`](docs/language.md) for the full reference.

## What's next

Concurrency is well underway: Erlang-style **processes** (`spawn`/`send`/`receive`/`self`)
run share-nothing as lightweight **green threads** on an M:N worker pool (≈`nproc`),
with reduction-counted preemption, selective `receive` + timeouts, and process
monitors (see [`examples/processes.blsp`](examples/processes.blsp)). **Distributed
nodes** are in too — two runtimes connect over TCP and message each other with
location-transparent `send`, remote monitors, closure-shipping, and an HMAC
handshake. Supervision is **userland** for now (`std/supervisor.blsp` over
`spawn`/`monitor`); a kernel-supervisor was tried and reverted.

The language core is now substantially complete — immutable maps, in-language
error handling, pattern matching, modules, the string/math/sequence libraries,
**dynamic variables**, an advisory set-theoretic **type checker**, and a
per-process tracing **GC** are all done. What remains of the core: a **package
manager**, self-hosting the CLI/REPL in Brood, and LSP Tier 2.

The editor milestones are already breaking ground as thin vertical slices: a
`ropey`-backed **rope kernel** + an immutable **buffer framework** (`std/buffer.blsp`)
for the editor data model; a serialisable **display protocol** (`std/display.blsp`)
with a terminal frontend, demoed end-to-end by `nest observe` (an Erlang-observer-style
process viewer). Still ahead: the editor app itself, a **server/daemon mode** so
other editor instances can attach remotely, and eventually a **web frontend**.

The full plan is in [`docs/roadmap.md`](docs/roadmap.md).

## Project layout

```
crates/lisp    the language: reader, evaluator, builtins, value model, scheduler, nodes
crates/cli     the `brood` binary: the language — REPL, file runner, `--test`
crates/nest    the `nest` binary: project tooling — `new`, `test`, `run`, `doc`, `format`, …
crates/lsp     the `brood-lsp` binary: the language server
crates/repl    the shared REPL, used by both `brood` and `nest`
std/           the prelude + opt-in modules (test, project, docs, buffer, display, …), in Brood
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

MIT.
