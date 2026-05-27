# mylisp

A small, dynamic **Lisp implemented in Rust**, built for one purpose: to be the
language a modern, Emacs-like text editor is *written in* — so that a running
editor can redefine its own behaviour on the fly.

The editor itself comes later. This repository is currently the **language
core** (v0.1): a reader, a tree-walking evaluator with proper tail calls and
lexical closures, a set of builtins, and a REPL.

```clojure
(+ 1 2)                          ;=> 3

(defn square (x) (* x x))        ; defn is a macro, written in mylisp itself
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
cargo run -p cli path/to/program.lisp
```

In the REPL:

```
mylisp v0.1 — type an expression, Ctrl-D to exit
mylisp> (+ 1 2)
3
mylisp> (defn greet (name) (str "hello, " name))
greet
mylisp> (greet "world")
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
mylisp itself (`std/prelude.lisp`) on top of a small Rust kernel.

See [`docs/language.md`](docs/language.md) for the full reference.

## What's next

Dynamic variables, in-language error handling, maps, and a tracing GC complete
the language. Then: a rope-backed **editor data model**, a
serialisable **display protocol** with a fast native local frontend, a
**server/daemon mode** so other editor instances can attach remotely, and
eventually a **web frontend**.

The full plan is in [`docs/roadmap.md`](docs/roadmap.md).

## Project layout

```
crates/lisp    the language: reader, evaluator, builtins, value model
crates/cli     the `mylisp` binary: REPL + file runner
std/           the prelude, written in mylisp
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
