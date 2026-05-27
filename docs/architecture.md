# Architecture

## Why this project exists

The goal is a modern, Emacs-like text editor whose defining property is the one
that makes Emacs Emacs: **the editor is written in the language it hosts, and a
running editor can redefine itself on the fly.** To get there we first need the
language. mylisp is that language, implemented in Rust.

Two requirements shape everything:

1. **Locally it must feel fast and native.** One runtime, no mandatory network
   hop, native rendering.
2. **The same runtime must be able to act as a server for other editor
   instances**, and later expose a web-style frontend.

The architecture is built so those two are the *same* code path, not two ports.

## The seam: one runtime, frontend-as-a-protocol

```
            ┌─────────────────────────────────────────────────┐
            │                 one runtime process              │
            │                                                  │
   keys/    │   ┌──────────┐   ┌───────────┐   ┌───────────┐   │
   events ─▶│   │  Lisp    │◀─▶│  editor   │◀─▶│  display  │   │── display ops ─▶ frontend(s)
            │   │  core    │   │  model    │   │  protocol │   │◀─ input events ─
            │   │ (today)  │   │ (later)   │   │ (later)   │   │
            │   └──────────┘   └───────────┘   └───────────┘   │
            │         ▲                              ▲          │
            │         │                              │          │
            │   live redefinition            in-proc native    │
            │   (eval / load / REPL)         frontend + TCP/WS server
            └─────────────────────────────────────────────────┘
```

Two commitments we make from the start, even though only the left box exists today:

1. **The Lisp owns the editor.** Buffers, cursors, keymaps, commands, and UI
   will be Lisp values and Lisp functions. Rust supplies primitives (rope
   operations, drawing, I/O); *policy* lives in Lisp and is hot-swappable.

2. **The frontend is a protocol, not a library.** The display layer will emit a
   serialisable stream of "render this" operations and consume input events. The
   local native frontend implements that protocol *in-process* (the fast path);
   a remote or web frontend implements the *identical* protocol over a socket.
   That is what lets "fast native locally" and "server for other instances" be
   the same code.

Why this is achievable for free: because the editor is written in mylisp,
hot-reload is just *re-evaluating definitions into the live global
environment* — no host-language hot-reload machinery required.

## Crate layout

```
mylisp/
  Cargo.toml            workspace
  crates/
    lisp/               the language (this is the substance today)
      src/
        value.rs        Value enum, symbol interner, list/vector constructors
        reader.rs       text -> Value (recursive-descent parser)
        env.rs          lexical environment chain
        eval.rs         tree-walking evaluator + special forms + tail calls
        builtins.rs     functions implemented in Rust
        printer.rs      Value -> text (round-trips with the reader)
        error.rs        LispError / LispResult
        lib.rs          the `Interp` entry point; bundles the prelude
      tests/basic.rs    end-to-end language tests
    cli/                the `mylisp` binary: REPL + file runner
      src/main.rs
  std/
    prelude.lisp        standard helpers written in mylisp itself
  docs/                 you are here
```

Later milestones add `crates/editor`, `crates/server`, and a frontend crate;
the `lisp` crate stays the foundation.

## How evaluation works

`eval(expr, env)` is a tree-walker, but the load-bearing detail is that it is a
**`'tail: loop`, not plain recursion**. For any expression in *tail position* —
the last form of a body, a chosen `if`/`cond` branch, the call in a tail call —
the evaluator reassigns its `expr`/`env` locals and loops instead of recursing.
The upshot: deeply recursive mylisp code (the natural way to write loops in a
Lisp) does **not** grow the Rust call stack. The test suite proves this by
summing to 1,000,000 via tail recursion.

Dispatch order for a list form `(head ...)`:

1. If `head` is a symbol naming a **special form**, handle it directly
   (`quote`, `if`, `fn`, `let`, `def`, …). Special forms control evaluation of
   their arguments.
2. Otherwise evaluate `head` and the arguments, then **apply**: a `Native`
   builtin is called directly; a `Fn` closure binds its parameters in a child of
   its captured environment and its body's last form is evaluated in tail
   position.

Symbols are **interned** to `u32` ids (see `value.rs`), so lookups and equality
are integer operations and the spelling is stored once.

## Memory model (and the plan to change it)

Today heap values live behind `Rc` and environments use `RefCell` for interior
mutability. This is the simplest correct thing and keeps the borrow checker out
of the way while we move fast.

Its one real limitation: **reference cycles leak.** A closure captures its
defining environment; if that environment can reach the closure, the `Rc`
cycle is never freed. For a REPL and the early milestones this is fine. Before
editor sessions become long-lived we will migrate to a tracing GC
(`gc-arena` — the design behind the Piccolo Lua VM). The migration is contained
because *all heap construction goes through the helpers in `value.rs`*.

## Dependencies

v0.1 has **zero external crates** — pure `std`. That keeps the build hermetic
and the first version easy to reason about. Dependencies will arrive with the
features that need them: `ropey` (text rope) for the editor, `tokio` + `serde`
for the server/protocol, a line-editor crate for a nicer REPL.
