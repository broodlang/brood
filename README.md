# Brood

**Brood** is a small, dynamic **Lisp implemented in Rust**, built for one
purpose: to be the language a modern, Emacs-like text editor is *written in* —
so that a running editor can redefine its own behaviour on the fly.

Under the Lisp sits Erlang/OTP-style concurrency: a *brood* of cheap, supervised
processes that share nothing and talk by message passing. That swarm is where
the name comes from.

> **Name & tooling.** This project was formerly `mylisp`. It is now **Brood**,
> with a toolchain that mirrors Elixir's `mix`/`hex`: **`tend`** is the
> build/project tool (`tend new`, `tend build`, `tend test`, `tend repl`) and
> **`nectar`** is the package registry/manager (`nectar add <dep>`). The colony
> imagery is deliberate — you *tend* a brood and feed it *nectar*. These tools
> and the crate/binary rename aren't built yet; today you use `cargo` directly,
> as shown below.
>
> Brood source files carry the **`.blsp`** extension — a contraction of *Brood
> Lisp* (`.lisp` collides with Emacs' `lisp-mode`). Any `.blsp` file, or a
> reference to "blsp", means **Brood-language source**, as distinct from the Rust
> kernel.

The editor itself comes later. This repository is currently the **language
core** (v0.1): a reader, a tree-walking evaluator with proper tail calls and
lexical closures, a set of builtins, and a REPL.

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

# run the tests
cargo test

# start the REPL
cargo run -p cli

# run a program file
cargo run -p cli path/to/program.blsp
```

In the REPL (`cargo run -p cli`; the banner and prompt will read `brood` once the
binary is renamed — today they still say `mylisp`):

```
brood v0.1 — type an expression, Ctrl-D to exit
brood> (+ 1 2)
3
brood> (defn greet (name) (str "hello, " name))
greet
brood> (greet "world")
"hello, world"
```

## What works today

Lexically-scoped closures, proper tail calls, `def`/`defn`/`set!`/`let`/`fn`,
`if`/`when`/`unless`/`cond`, `and`/`or`/`while`, **macros** (`defmacro` +
Clojure-style `` ` ``/`~`/`~@` quasiquote, `macroexpand`, `gensym`), integers &
floats with overflow-checked arithmetic, strings, symbols, keywords, cons-cell
lists, `[ ]` vectors, higher-order functions (`map`/`filter`/`reduce`/`apply`),
and the self-hosting trio `eval`/`read-string`/`load`. Parameter lists are
written as lists (`(x y)` — code is lists; vectors are data) and support
`&optional` (with defaults) and `& rest`. `defn`, the operators (`+`, `<`, …),
the sequence library, and the `->`/`->>` threading macros are all defined in
Brood itself (`std/prelude.blsp`) on top of a small Rust kernel.

See [`docs/language.md`](docs/language.md) for the full reference.

## What's next

Concurrency has begun: Erlang-style **processes** (`spawn`/`send`/`receive`/`self`)
run share-nothing on real threads and talk by message passing (see
[`examples/processes.blsp`](examples/processes.blsp)); making them lightweight
green threads on a worker pool is in progress.

Dynamic variables, in-language error handling, maps, and a tracing GC complete
the language. Then: a rope-backed **editor data model**, a
serialisable **display protocol** with a fast native local frontend, a
**server/daemon mode** so other editor instances can attach remotely, and
eventually a **web frontend**.

The full plan is in [`docs/roadmap.md`](docs/roadmap.md).

## Project layout

```
crates/lisp    the language: reader, evaluator, builtins, value model
crates/cli     the binary: REPL + file runner (to be renamed `brood`; `tend`/`nectar` will wrap it)
std/           the prelude, written in Brood
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
